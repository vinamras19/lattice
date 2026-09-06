use super::frame::MessageCodec;
use super::message::Message;
use crate::error::{LatticeError, Result};
use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_util::codec::Framed;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub trait Handler: Send + Sync + 'static {
    fn on_write(&self, series: u64, points: Vec<(i64, f64)>) -> bool;
    fn on_query(&self, series: u64, start: i64, end: i64) -> Vec<(i64, f64)>;
    fn on_raft(&self, _bytes: Vec<u8>) {}
}

pub struct Transport {
    node_id: u64,
    addr: SocketAddr,
    next_req: AtomicU64,
    handler: Arc<dyn Handler>,
}

impl Transport {
    pub fn new(node_id: u64, addr: SocketAddr, handler: Arc<dyn Handler>) -> Self {
        Self { node_id, addr, next_req: AtomicU64::new(1), handler }
    }

    pub fn next_req_id(&self) -> u64 {
        self.next_req.fetch_add(1, Ordering::Relaxed)
    }

    pub async fn serve(self: Arc<Self>) -> Result<()> {
        let listener = TcpListener::bind(self.addr).await?;
        self.serve_bound(listener).await
    }

    pub async fn serve_bound(self: Arc<Self>, listener: TcpListener) -> Result<()> {
        let bound = listener.local_addr()?;
        tracing::info!(node = self.node_id, addr = %bound, "cluster transport listening");
        loop {
            let (stream, peer) = listener.accept().await?;
            let this = Arc::clone(&self);
            tokio::spawn(async move {
                if let Err(e) = this.handle_conn(stream).await {
                    tracing::debug!(%peer, error = %e, "connection closed");
                }
            });
        }
    }

    async fn handle_conn(&self, stream: TcpStream) -> Result<()> {
        let mut framed = Framed::new(stream, MessageCodec::new());
        while let Some(msg) = framed.next().await {
            if let Some(resp) = self.dispatch(msg?) {
                framed.send(resp).await?;
            }
        }
        Ok(())
    }

    fn dispatch(&self, msg: Message) -> Option<Message> {
        match msg {
            Message::Ping => Some(Message::Pong),
            Message::Write { req_id, series, points } => {
                let ok = self.handler.on_write(series, points);
                Some(Message::WriteAck { req_id, ok })
            }
            Message::Query { req_id, series, start, end } => {
                let points = self.handler.on_query(series, start, end);
                Some(Message::QueryResult { req_id, points })
            }
            Message::Raft(bytes) => {
                self.handler.on_raft(bytes);
                None
            }
            _ => None,
        }
    }

    pub async fn request(&self, peer: SocketAddr, msg: Message) -> Result<Message> {
        let stream = TcpStream::connect(peer).await?;
        let mut framed = Framed::new(stream, MessageCodec::new());
        framed.send(msg).await?;
        match timeout(REQUEST_TIMEOUT, framed.next()).await {
            Ok(Some(resp)) => resp,
            Ok(None) => Err(LatticeError::Network("peer closed before reply".into())),
            Err(_) => Err(LatticeError::Network("request timed out".into())),
        }
    }

    pub async fn send_oneway(&self, peer: SocketAddr, msg: Message) -> Result<()> {
        let stream = TcpStream::connect(peer).await?;
        let mut framed = Framed::new(stream, MessageCodec::new());
        framed.send(msg).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct MapHandler {
        data: Mutex<HashMap<u64, Vec<(i64, f64)>>>,
    }

    impl Handler for MapHandler {
        fn on_write(&self, series: u64, points: Vec<(i64, f64)>) -> bool {
            self.data.lock().unwrap().entry(series).or_default().extend(points);
            true
        }

        fn on_query(&self, series: u64, start: i64, end: i64) -> Vec<(i64, f64)> {
            self.data
                .lock()
                .unwrap()
                .get(&series)
                .map(|v| v.iter().copied().filter(|&(ts, _)| ts >= start && ts < end).collect())
                .unwrap_or_default()
        }
    }

    struct NoopHandler;
    impl Handler for NoopHandler {
        fn on_write(&self, _: u64, _: Vec<(i64, f64)>) -> bool {
            false
        }
        fn on_query(&self, _: u64, _: i64, _: i64) -> Vec<(i64, f64)> {
            Vec::new()
        }
    }

    #[tokio::test]
    async fn write_then_query_over_wire() {
        let server = Arc::new(Transport::new(
            1,
            "127.0.0.1:0".parse().unwrap(),
            Arc::new(MapHandler { data: Mutex::new(HashMap::new()) }),
        ));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound = listener.local_addr().unwrap();
        tokio::spawn(Arc::clone(&server).serve_bound(listener));

        let client = Arc::new(Transport::new(
            2,
            "127.0.0.1:0".parse().unwrap(),
            Arc::new(NoopHandler),
        ));

        let points = vec![(100, 1.5), (200, 2.5), (5000, 9.0)];
        let ack = client
            .request(bound, Message::Write { req_id: 1, series: 7, points: points.clone() })
            .await
            .unwrap();
        assert_eq!(ack, Message::WriteAck { req_id: 1, ok: true });

        let result = client
            .request(bound, Message::Query { req_id: 2, series: 7, start: 0, end: 1000 })
            .await
            .unwrap();
        assert_eq!(
            result,
            Message::QueryResult { req_id: 2, points: vec![(100, 1.5), (200, 2.5)] }
        );
    }
}