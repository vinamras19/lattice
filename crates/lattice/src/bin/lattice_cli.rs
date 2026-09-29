use lattice::cluster::protocol::message::Message;
use lattice::cluster::protocol::transport::{Handler, Transport};
use lattice::Result;
use std::io::{BufRead, Write};
use std::net::SocketAddr;
use std::sync::Arc;

struct ClientHandler;
impl Handler for ClientHandler {
    fn on_write(&self, _: u64, _: Vec<(i64, f64)>) -> bool {
        false
    }
    fn on_query(&self, _: u64, _: i64, _: i64) -> Vec<(i64, f64)> {
        Vec::new()
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let server: SocketAddr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:7700".to_string())
        .parse()
        .expect("usage: lattice_cli <host:port>");

    let client = Arc::new(Transport::new(0, "127.0.0.1:0".parse().unwrap(), Arc::new(ClientHandler)));
    println!("lattice cli -> {server}");
    println!("commands: write <series> <ts> <value> | query <series> <start> <end> | ping | exit");

    let stdin = std::io::stdin();
    let mut line = String::new();
    loop {
        print!("lattice> ");
        std::io::stdout().flush().ok();
        line.clear();
        if stdin.lock().read_line(&mut line)? == 0 {
            break;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        match parts.as_slice() {
            ["exit"] | ["quit"] => break,
            ["ping"] => match client.request(server, Message::Ping).await {
                Ok(Message::Pong) => println!("pong"),
                Ok(other) => println!("unexpected: {other:?}"),
                Err(e) => println!("error: {e}"),
            },
            ["write", s, ts, v] => match (s.parse::<u64>(), ts.parse::<i64>(), v.parse::<f64>()) {
                (Ok(series), Ok(ts), Ok(value)) => {
                    let msg = Message::Write { req_id: client.next_req_id(), series, points: vec![(ts, value)] };
                    match client.request(server, msg).await {
                        Ok(Message::WriteAck { ok, .. }) => println!("ack ok={ok}"),
                        Ok(other) => println!("unexpected: {other:?}"),
                        Err(e) => println!("error: {e}"),
                    }
                }
                _ => println!("usage: write <series:u64> <ts:i64> <value:f64>"),
            },
            ["query", s, start, end] => match (s.parse::<u64>(), start.parse::<i64>(), end.parse::<i64>()) {
                (Ok(series), Ok(start), Ok(end)) => {
                    let msg = Message::Query { req_id: client.next_req_id(), series, start, end };
                    match client.request(server, msg).await {
                        Ok(Message::QueryResult { points, .. }) => {
                            println!("{} points", points.len());
                            for (ts, value) in points {
                                println!("  {ts}  {value}");
                            }
                        }
                        Ok(other) => println!("unexpected: {other:?}"),
                        Err(e) => println!("error: {e}"),
                    }
                }
                _ => println!("usage: query <series:u64> <start:i64> <end:i64>"),
            },
            [] => {}
            _ => println!("unknown command"),
        }
    }
    Ok(())
}