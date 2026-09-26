use lattice::api::ApiServer;
use lattice::cluster::consensus::{discard_logger, RaftNode};
use lattice::cluster::driver::{RaftDriver, RaftHandle, RaftShared};
use lattice::cluster::protocol::transport::{Handler, Transport};
use lattice::cluster::StorageHandler;
use lattice::config::Config;
use lattice::storage::Storage;
use lattice::{LatticeError, Result};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

#[tokio::main]
async fn main() -> Result<()> {
    lattice::telemetry::init();

    let config_path = std::env::args().nth(1).unwrap_or_else(|| "lattice.toml".into());
    let config = Config::load(&config_path)?;
    let storage = Arc::new(Storage::open(&config)?);
    tracing::info!(node = config.node_id, dir = %config.data_dir, "storage opened");

    let listen: SocketAddr = config
        .listen_addr
        .parse()
        .map_err(|e| LatticeError::Config(format!("bad listen_addr {}: {e}", config.listen_addr)))?;
    let api_addr: SocketAddr = config
        .api_addr
        .parse()
        .map_err(|e| LatticeError::Config(format!("bad api_addr {}: {e}", config.api_addr)))?;

    // Single node unless peers are configured. With peers, writes route through Raft
    // and the driver applies committed entries to every replica's storage.
    let handler: Arc<dyn Handler>;
    let mut driver_args = None;
    if config.peers.is_empty() {
        handler = Arc::new(StorageHandler::new(Arc::clone(&storage)));
    } else {
        let voters: Vec<u64> = config.peers.iter().map(|p| p.id).collect();
        let mut peer_addrs: HashMap<u64, SocketAddr> = HashMap::new();
        for p in &config.peers {
            if p.id == config.node_id {
                continue;
            }
            let addr: SocketAddr = p
                .addr
                .parse()
                .map_err(|e| LatticeError::Config(format!("bad peer addr {}: {e}", p.addr)))?;
            peer_addrs.insert(p.id, addr);
        }
        let (handle, rx) = RaftHandle::channel();
        let shared = Arc::new(Mutex::new(RaftShared::default()));
        let node = RaftNode::new(config.node_id, &voters, &discard_logger())?;
        handler = Arc::new(StorageHandler::with_raft(
            Arc::clone(&storage),
            handle,
            Arc::clone(&shared),
        ));
        driver_args = Some((node, rx, shared, peer_addrs));
    }

    let transport = Arc::new(Transport::new(config.node_id, listen, handler));

    if let Some((node, rx, shared, peer_addrs)) = driver_args {
        tracing::info!(node = config.node_id, peers = peer_addrs.len(), "raft cluster mode");
        RaftDriver::spawn(
            node,
            Arc::clone(&transport),
            peer_addrs,
            rx,
            shared,
            Some(Arc::clone(&storage)),
        );
    }

    let api = Arc::new(ApiServer::new(config.node_id, Arc::clone(&storage)));
    tokio::spawn(async move {
        if let Err(e) = api.serve(api_addr).await {
            tracing::error!(error = %e, "api server stopped");
        }
    });

    transport.serve().await
}