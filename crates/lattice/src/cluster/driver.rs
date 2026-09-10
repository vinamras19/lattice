use crate::cluster::consensus::{decode_message, encode_message, RaftNode};
use crate::cluster::protocol::message::Message as WireMessage;
use crate::cluster::protocol::transport::{Handler, Transport};
use crate::storage::{Point, Storage};
use raft::prelude::Message as RaftMessage;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

const TICK: Duration = Duration::from_millis(100);

pub enum Command {
    Step(RaftMessage),
    Propose(Vec<u8>),
    Campaign,
}

#[derive(Clone)]
pub struct RaftHandle {
    tx: mpsc::UnboundedSender<Command>,
}

impl RaftHandle {
    pub fn channel() -> (RaftHandle, mpsc::UnboundedReceiver<Command>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (RaftHandle { tx }, rx)
    }

    pub fn step(&self, msg: RaftMessage) {
        let _ = self.tx.send(Command::Step(msg));
    }

    pub fn propose(&self, data: Vec<u8>) {
        let _ = self.tx.send(Command::Propose(data));
    }

    pub fn campaign(&self) {
        let _ = self.tx.send(Command::Campaign);
    }
}

// A single point written through the Raft log: series + timestamp + value.
pub fn encode_write(series: u64, ts: i64, value: f64) -> Vec<u8> {
    let mut b = Vec::with_capacity(24);
    b.extend_from_slice(&series.to_le_bytes());
    b.extend_from_slice(&ts.to_le_bytes());
    b.extend_from_slice(&value.to_bits().to_le_bytes());
    b
}

fn decode_write(data: &[u8]) -> Option<(u64, i64, f64)> {
    if data.len() != 24 {
        return None;
    }
    let series = u64::from_le_bytes(data[0..8].try_into().ok()?);
    let ts = i64::from_le_bytes(data[8..16].try_into().ok()?);
    let bits = u64::from_le_bytes(data[16..24].try_into().ok()?);
    Some((series, ts, f64::from_bits(bits)))
}

// Observable view of a driver's progress, updated after every Ready.
#[derive(Default)]
pub struct RaftShared {
    pub applied: Vec<Vec<u8>>,
    pub is_leader: bool,
}

// Handler that decodes inbound Raft frames and steps them into a driver.
pub struct RaftForwardHandler {
    handle: RaftHandle,
}

impl RaftForwardHandler {
    pub fn new(handle: RaftHandle) -> Self {
        Self { handle }
    }
}

impl Handler for RaftForwardHandler {
    fn on_write(&self, _series: u64, _points: Vec<(i64, f64)>) -> bool {
        false
    }

    fn on_query(&self, _series: u64, _start: i64, _end: i64) -> Vec<(i64, f64)> {
        Vec::new()
    }

    fn on_raft(&self, bytes: Vec<u8>) {
        match decode_message(&bytes) {
            Ok(msg) => self.handle.step(msg),
            Err(e) => tracing::debug!(error = %e, "dropped malformed raft message"),
        }
    }
}

pub struct RaftDriver;

impl RaftDriver {
    pub fn spawn(
        node: RaftNode,
        transport: Arc<Transport>,
        peers: HashMap<u64, SocketAddr>,
        commands: mpsc::UnboundedReceiver<Command>,
        shared: Arc<Mutex<RaftShared>>,
        storage: Option<Arc<Storage>>,
    ) {
        tokio::spawn(run(node, transport, peers, commands, shared, storage));
    }
}

async fn run(
    mut node: RaftNode,
    transport: Arc<Transport>,
    peers: HashMap<u64, SocketAddr>,
    mut commands: mpsc::UnboundedReceiver<Command>,
    shared: Arc<Mutex<RaftShared>>,
    storage: Option<Arc<Storage>>,
) {
    let mut tick = tokio::time::interval(TICK);

    loop {
        tokio::select! {
            cmd = commands.recv() => {
                match cmd {
                    Some(Command::Step(msg)) => { let _ = node.step(msg); }
                    Some(Command::Propose(data)) => { let _ = node.propose(data); }
                    Some(Command::Campaign) => { let _ = node.campaign(); }
                    None => break, // every handle dropped; shut the driver down
                }
            }
            _ = tick.tick() => {
                node.tick();
            }
        }

        for msg in node.process_ready() {
            if let Some(&addr) = peers.get(&msg.get_to()) {
                match encode_message(&msg) {
                    Ok(bytes) => {
                        let transport = Arc::clone(&transport);
                        tokio::spawn(async move {
                            let _ = transport.send_oneway(addr, WireMessage::Raft(bytes)).await;
                        });
                    }
                    Err(e) => tracing::debug!(error = %e, "raft encode failed"),
                }
            }
        }

        apply_committed(&node, &storage, &shared);
    }
}

fn apply_committed(node: &RaftNode, storage: &Option<Arc<Storage>>, shared: &Arc<Mutex<RaftShared>>) {
    // Record newly committed entries under the lock, then release before any I/O.
    let new_entries: Vec<Vec<u8>> = {
        let mut s = shared.lock().unwrap();
        s.is_leader = node.is_leader();

        let applied = node.applied();
        let start = s.applied.len();
        let fresh: Vec<Vec<u8>> = applied[start..].to_vec();
        for data in &fresh {
            s.applied.push(data.clone());
        }
        fresh
    };

    if let Some(storage) = storage {
        for data in &new_entries {
            if let Some((series, ts, value)) = decode_write(data) {
                if let Err(e) = storage.write(Point { series, ts, value }) {
                    tracing::error!(error = %e, "raft apply: storage write failed");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::consensus::discard_logger;
    use tokio::net::TcpListener;
    use tokio::time::sleep;

    const MSG: &[u8] = b"over-the-wire";

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

    #[tokio::test]
    async fn replicates_over_tcp() {
        let ids = [1u64, 2, 3];
        let logger = discard_logger();

        // bind every listener first so we know each node's address up front
        let mut listeners = Vec::new();
        let mut addrs: HashMap<u64, SocketAddr> = HashMap::new();
        for &id in &ids {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            addrs.insert(id, listener.local_addr().unwrap());
            listeners.push((id, listener));
        }

        let mut handles: HashMap<u64, RaftHandle> = HashMap::new();
        let mut shareds: HashMap<u64, Arc<Mutex<RaftShared>>> = HashMap::new();

        for (id, listener) in listeners {
            let (tx, rx) = mpsc::unbounded_channel();
            let handle = RaftHandle { tx };
            let shared = Arc::new(Mutex::new(RaftShared::default()));

            let transport = Arc::new(Transport::new(
                id,
                addrs[&id],
                Arc::new(RaftForwardHandler::new(handle.clone())),
            ));
            tokio::spawn(Arc::clone(&transport).serve_bound(listener));

            let node = RaftNode::new(id, &ids, &logger).unwrap();
            RaftDriver::spawn(node, transport, addrs.clone(), rx, Arc::clone(&shared), None);

            handles.insert(id, handle);
            shareds.insert(id, shared);
        }

        // give the listeners a moment to start accepting
        sleep(Duration::from_millis(200)).await;

        // force node 1 to stand for election
        handles[&1].campaign();

        let elected = wait_until(Duration::from_secs(5), || {
            shareds.values().any(|s| s.lock().unwrap().is_leader)
        })
        .await;
        assert!(elected, "no leader elected over TCP");

        let leader_id = *shareds
            .iter()
            .find(|(_, s)| s.lock().unwrap().is_leader)
            .map(|(id, _)| id)
            .unwrap();
        handles[&leader_id].propose(MSG.to_vec());

        let replicated = wait_until(Duration::from_secs(5), || {
            shareds
                .values()
                .all(|s| s.lock().unwrap().applied.iter().any(|d| d.as_slice() == MSG))
        })
        .await;
        assert!(replicated, "entry not replicated to all nodes over TCP");
    }

    #[tokio::test]
    async fn replicates_writes_to_storage() {
        use crate::config::Config;

        let ids = [1u64, 2, 3];
        let logger = discard_logger();

        let mut listeners = Vec::new();
        let mut addrs: HashMap<u64, SocketAddr> = HashMap::new();
        for &id in &ids {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            addrs.insert(id, listener.local_addr().unwrap());
            listeners.push((id, listener));
        }

        let mut handles: HashMap<u64, RaftHandle> = HashMap::new();
        let mut shareds: HashMap<u64, Arc<Mutex<RaftShared>>> = HashMap::new();
        let mut stores: HashMap<u64, Arc<Storage>> = HashMap::new();
        let mut dirs: Vec<std::path::PathBuf> = Vec::new();

        for (id, listener) in listeners {
            let dir = std::env::temp_dir().join(format!("lattice_raft_store_{id}"));
            let _ = std::fs::remove_dir_all(&dir);
            let config = Config { data_dir: dir.to_string_lossy().into_owned(), ..Default::default() };
            let storage = Arc::new(Storage::open(&config).unwrap());
            dirs.push(dir);

            let (tx, rx) = mpsc::unbounded_channel();
            let handle = RaftHandle { tx };
            let shared = Arc::new(Mutex::new(RaftShared::default()));

            let transport = Arc::new(Transport::new(
                id,
                addrs[&id],
                Arc::new(RaftForwardHandler::new(handle.clone())),
            ));
            tokio::spawn(Arc::clone(&transport).serve_bound(listener));

            let node = RaftNode::new(id, &ids, &logger).unwrap();
            RaftDriver::spawn(
                node,
                transport,
                addrs.clone(),
                rx,
                Arc::clone(&shared),
                Some(Arc::clone(&storage)),
            );

            handles.insert(id, handle);
            shareds.insert(id, shared);
            stores.insert(id, storage);
        }

        sleep(Duration::from_millis(200)).await;
        handles[&1].campaign();

        let elected = wait_until(Duration::from_secs(5), || {
            shareds.values().any(|s| s.lock().unwrap().is_leader)
        })
        .await;
        assert!(elected, "no leader elected");

        let leader_id = *shareds
            .iter()
            .find(|(_, s)| s.lock().unwrap().is_leader)
            .map(|(id, _)| id)
            .unwrap();
        handles[&leader_id].propose(encode_write(7, 100, 1.5));

        let replicated = wait_until(Duration::from_secs(5), || {
            stores
                .values()
                .all(|st| st.query(7, 0, 1000).map(|v| v == vec![(100, 1.5)]).unwrap_or(false))
        })
        .await;
        assert!(replicated, "write not replicated to all nodes' storage");

        for dir in dirs {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}