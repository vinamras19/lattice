pub mod consensus;
pub mod driver;
pub mod protocol;

use crate::cluster::consensus::decode_message;
use crate::cluster::driver::{encode_write, RaftHandle, RaftShared};
use crate::storage::{Point, Storage};
use protocol::transport::Handler;
use std::sync::{Arc, Mutex};

struct RaftRef {
    handle: RaftHandle,
    shared: Arc<Mutex<RaftShared>>,
}

pub struct StorageHandler {
    storage: Arc<Storage>,
    raft: Option<RaftRef>,
}

impl StorageHandler {
    pub fn new(storage: Arc<Storage>) -> Self {
        Self { storage, raft: None }
    }

    pub fn with_raft(storage: Arc<Storage>, handle: RaftHandle, shared: Arc<Mutex<RaftShared>>) -> Self {
        Self { storage, raft: Some(RaftRef { handle, shared }) }
    }
}

impl Handler for StorageHandler {
    fn on_write(&self, series: u64, points: Vec<(i64, f64)>) -> bool {
        match &self.raft {
            // single node: apply directly
            None => {
                let batch: Vec<Point> = points
                    .into_iter()
                    .map(|(ts, value)| Point { series, ts, value })
                    .collect();
                match self.storage.write_batch(&batch) {
                    Ok(()) => true,
                    Err(e) => {
                        tracing::error!(series, error = %e, "local write failed");
                        false
                    }
                }
            }
            // clustered: only the leader accepts. The entry replicates through the
            // log and is applied to every replica's storage on commit.
            Some(r) => {
                if !r.shared.lock().unwrap().is_leader {
                    return false;
                }
                for (ts, value) in points {
                    r.handle.propose(encode_write(series, ts, value));
                }
                true
            }
        }
    }

    fn on_query(&self, series: u64, start: i64, end: i64) -> Vec<(i64, f64)> {
        self.storage.query(series, start, end).unwrap_or_default()
    }

    fn on_raft(&self, bytes: Vec<u8>) {
        if let Some(r) = &self.raft {
            match decode_message(&bytes) {
                Ok(msg) => r.handle.step(msg),
                Err(e) => tracing::debug!(error = %e, "dropped malformed raft message"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StorageHandler;
    use crate::cluster::consensus::{discard_logger, RaftNode};
    use crate::cluster::driver::{RaftDriver, RaftHandle, RaftShared};
    use crate::cluster::protocol::message::Message;
    use crate::cluster::protocol::transport::{Handler, Transport};
    use crate::config::Config;
    use crate::storage::Storage;
    use std::collections::HashMap;
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;
    use tokio::time::{sleep, Duration};

    async fn wait_until<F: Fn() -> bool>(limit: Duration, cond: F) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < limit {
            if cond() {
                return true;
            }
            sleep(Duration::from_millis(50)).await;
        }
        cond()
    }

    struct ClientHandler;
    impl Handler for ClientHandler {
        fn on_write(&self, _: u64, _: Vec<(i64, f64)>) -> bool {
            false
        }
        fn on_query(&self, _: u64, _: i64, _: i64) -> Vec<(i64, f64)> {
            Vec::new()
        }
    }

    #[tokio::test]
    async fn client_write_replicates_through_raft() {
        let ids = [1u64, 2, 3];
        let logger = discard_logger();

        let mut listeners = Vec::new();
        let mut addrs: HashMap<u64, SocketAddr> = HashMap::new();
        for &id in &ids {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            addrs.insert(id, listener.local_addr().unwrap());
            listeners.push((id, listener));
        }

        let mut shareds: HashMap<u64, Arc<Mutex<RaftShared>>> = HashMap::new();
        let mut stores: HashMap<u64, Arc<Storage>> = HashMap::new();
        let mut dirs: Vec<std::path::PathBuf> = Vec::new();
        let mut node1_handle: Option<RaftHandle> = None;

        for (id, listener) in listeners {
            let dir = std::env::temp_dir().join(format!("lattice_cluster_store_{id}"));
            let _ = std::fs::remove_dir_all(&dir);
            let config = Config { data_dir: dir.to_string_lossy().into_owned(), ..Default::default() };
            let storage = Arc::new(Storage::open(&config).unwrap());
            dirs.push(dir);

            let (handle, rx) = RaftHandle::channel();
            let shared = Arc::new(Mutex::new(RaftShared::default()));

            let transport = Arc::new(Transport::new(
                id,
                addrs[&id],
                Arc::new(StorageHandler::with_raft(
                    Arc::clone(&storage),
                    handle.clone(),
                    Arc::clone(&shared),
                )),
            ));
            tokio::spawn(Arc::clone(&transport).serve_bound(listener));

            let node = RaftNode::new(id, &ids, &logger).unwrap();
            RaftDriver::spawn(node, transport, addrs.clone(), rx, Arc::clone(&shared), Some(Arc::clone(&storage)));

            if id == 1 {
                node1_handle = Some(handle);
            }
            shareds.insert(id, shared);
            stores.insert(id, storage);
        }

        sleep(Duration::from_millis(200)).await;
        node1_handle.unwrap().campaign();

        let elected = wait_until(Duration::from_secs(5), || {
            shareds.get(&1).unwrap().lock().unwrap().is_leader
        })
        .await;
        assert!(elected, "node 1 did not become leader");

        let client = Arc::new(Transport::new(0, "127.0.0.1:0".parse().unwrap(), Arc::new(ClientHandler)));

        // write to the leader -> accepted, and replicated to every node's storage
        let ack = client
            .request(addrs[&1], Message::Write { req_id: 1, series: 7, points: vec![(100, 1.5)] })
            .await
            .unwrap();
        assert_eq!(ack, Message::WriteAck { req_id: 1, ok: true });

        let replicated = wait_until(Duration::from_secs(5), || {
            stores
                .values()
                .all(|st| st.query(7, 0, 1000).map(|v| v == vec![(100, 1.5)]).unwrap_or(false))
        })
        .await;
        assert!(replicated, "write not replicated to all replicas");

        // write to a follower -> rejected
        let follower = if shareds.get(&2).unwrap().lock().unwrap().is_leader { 3 } else { 2 };
        let reject = client
            .request(addrs[&follower], Message::Write { req_id: 2, series: 7, points: vec![(200, 2.5)] })
            .await
            .unwrap();
        assert_eq!(reject, Message::WriteAck { req_id: 2, ok: false });

        for dir in dirs {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}