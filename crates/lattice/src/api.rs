use crate::error::Result;
use crate::storage::Storage;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub struct ApiServer {
    node_id: u64,
    storage: Arc<Storage>,
    start: Instant,
}

impl ApiServer {
    pub fn new(node_id: u64, storage: Arc<Storage>) -> Self {
        Self { node_id, storage, start: Instant::now() }
    }

    pub async fn serve(self: Arc<Self>, addr: SocketAddr) -> Result<()> {
        let listener = TcpListener::bind(addr).await?;
        self.serve_bound(listener).await
    }

    pub async fn serve_bound(self: Arc<Self>, listener: TcpListener) -> Result<()> {
        tracing::info!(addr = %listener.local_addr()?, "api listening");
        loop {
            let (stream, _) = listener.accept().await?;
            let this = Arc::clone(&self);
            tokio::spawn(async move {
                if let Err(e) = this.handle(stream).await {
                    tracing::debug!(error = %e, "api request failed");
                }
            });
        }
    }

    async fn handle(&self, mut stream: TcpStream) -> Result<()> {
        let mut buf = [0u8; 1024];
        let n = stream.read(&mut buf).await?;
        let req = String::from_utf8_lossy(&buf[..n]);
        let path = req.split_whitespace().nth(1).unwrap_or("/");

        let (status, ctype, body) = match path {
            "/health" => (200, "application/json", self.health()),
            "/stats" => (200, "application/json", self.stats()),
            "/metrics" => (200, "text/plain; version=0.0.4", self.metrics()),
            _ => (404, "text/plain", "not found".to_string()),
        };

        let reason = if status == 200 { "OK" } else { "Not Found" };
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await?;
        Ok(())
    }

    fn health(&self) -> String {
        format!(
            "{{\"status\":\"healthy\",\"node\":{},\"uptime_secs\":{}}}",
            self.node_id,
            self.start.elapsed().as_secs()
        )
    }

    fn stats(&self) -> String {
        let s = self.storage.stats();
        format!(
            "{{\"node\":{},\"series\":{},\"segments\":{},\"memtable_points\":{}}}",
            self.node_id, s.series, s.segments, s.memtable_points
        )
    }

    fn metrics(&self) -> String {
        let s = self.storage.stats();
        let mut out = String::new();
        out.push_str("# TYPE lattice_uptime_seconds counter\n");
        out.push_str(&format!("lattice_uptime_seconds {}\n", self.start.elapsed().as_secs()));
        out.push_str("# TYPE lattice_series gauge\n");
        out.push_str(&format!("lattice_series {}\n", s.series));
        out.push_str("# TYPE lattice_segments gauge\n");
        out.push_str(&format!("lattice_segments {}\n", s.segments));
        out.push_str("# TYPE lattice_memtable_points gauge\n");
        out.push_str(&format!("lattice_memtable_points {}\n", s.memtable_points));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[tokio::test]
    async fn health_endpoint() {
        let dir = std::env::temp_dir().join("lattice_api_test");
        let _ = std::fs::remove_dir_all(&dir);
        let config = Config { data_dir: dir.to_string_lossy().into_owned(), ..Default::default() };
        let storage = Arc::new(Storage::open(&config).unwrap());
        let api = Arc::new(ApiServer::new(1, storage));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(Arc::clone(&api).serve_bound(listener));

        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream.write_all(b"GET /health HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
        let mut buf = vec![0u8; 512];
        let n = stream.read(&mut buf).await.unwrap();
        let resp = String::from_utf8_lossy(&buf[..n]);

        assert!(resp.contains("\"status\":\"healthy\""), "unexpected: {resp}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}