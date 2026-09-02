use crate::error::{LatticeError, Result};

const PING: u8 = 1;
const PONG: u8 = 2;
const WRITE: u8 = 3;
const WRITE_ACK: u8 = 4;
const QUERY: u8 = 5;
const QUERY_RESULT: u8 = 6;
const RAFT: u8 = 7;

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Ping,
    Pong,
    Write { req_id: u64, series: u64, points: Vec<(i64, f64)> },
    WriteAck { req_id: u64, ok: bool },
    Query { req_id: u64, series: u64, start: i64, end: i64 },
    QueryResult { req_id: u64, points: Vec<(i64, f64)> },
    Raft(Vec<u8>),
}

impl Message {
    pub fn serialize(&self) -> Vec<u8> {
        let mut b = Vec::new();
        match self {
            Message::Ping => b.push(PING),
            Message::Pong => b.push(PONG),
            Message::Write { req_id, series, points } => {
                b.push(WRITE);
                b.extend_from_slice(&req_id.to_le_bytes());
                b.extend_from_slice(&series.to_le_bytes());
                write_points(&mut b, points);
            }
            Message::WriteAck { req_id, ok } => {
                b.push(WRITE_ACK);
                b.extend_from_slice(&req_id.to_le_bytes());
                b.push(*ok as u8);
            }
            Message::Query { req_id, series, start, end } => {
                b.push(QUERY);
                b.extend_from_slice(&req_id.to_le_bytes());
                b.extend_from_slice(&series.to_le_bytes());
                b.extend_from_slice(&start.to_le_bytes());
                b.extend_from_slice(&end.to_le_bytes());
            }
            Message::QueryResult { req_id, points } => {
                b.push(QUERY_RESULT);
                b.extend_from_slice(&req_id.to_le_bytes());
                write_points(&mut b, points);
            }
            Message::Raft(bytes) => {
                b.push(RAFT);
                b.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                b.extend_from_slice(bytes);
            }
        }
        b
    }

    pub fn deserialize(data: &[u8]) -> Result<Message> {
        let mut c = Cursor { data, pos: 0 };
        let msg = match c.u8()? {
            PING => Message::Ping,
            PONG => Message::Pong,
            WRITE => Message::Write {
                req_id: c.u64()?,
                series: c.u64()?,
                points: c.points()?,
            },
            WRITE_ACK => Message::WriteAck {
                req_id: c.u64()?,
                ok: c.u8()? != 0,
            },
            QUERY => Message::Query {
                req_id: c.u64()?,
                series: c.u64()?,
                start: c.i64()?,
                end: c.i64()?,
            },
            QUERY_RESULT => Message::QueryResult {
                req_id: c.u64()?,
                points: c.points()?,
            },
            RAFT => Message::Raft(c.bytes()?),
            other => return Err(LatticeError::Corrupt(format!("unknown message tag {other}"))),
        };
        Ok(msg)
    }
}

fn write_points(b: &mut Vec<u8>, points: &[(i64, f64)]) {
    b.extend_from_slice(&(points.len() as u32).to_le_bytes());
    for &(ts, v) in points {
        b.extend_from_slice(&ts.to_le_bytes());
        b.extend_from_slice(&v.to_bits().to_le_bytes());
    }
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.pos + n > self.data.len() {
            return Err(LatticeError::Corrupt("short message".into()));
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_bits(self.u64()?))
    }

    fn points(&mut self) -> Result<Vec<(i64, f64)>> {
        let n = self.u32()? as usize;
        // 16 bytes per point; reject a count the remaining data cannot hold
        if n.saturating_mul(16) > self.data.len() - self.pos {
            return Err(LatticeError::Corrupt("points count exceeds message".into()));
        }
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push((self.i64()?, self.f64()?));
        }
        Ok(v)
    }

    fn bytes(&mut self) -> Result<Vec<u8>> {
        let n = self.u32()? as usize;
        Ok(self.take(n)?.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(m: Message) {
        assert_eq!(Message::deserialize(&m.serialize()).unwrap(), m);
    }

    #[test]
    fn messages_roundtrip() {
        roundtrip(Message::Ping);
        roundtrip(Message::Pong);
        roundtrip(Message::Write { req_id: 9, series: 7, points: vec![(1, 1.5), (2, -3.0)] });
        roundtrip(Message::WriteAck { req_id: 9, ok: true });
        roundtrip(Message::Query { req_id: 3, series: 7, start: 0, end: 100 });
        roundtrip(Message::QueryResult { req_id: 3, points: vec![(10, 2.0)] });
        roundtrip(Message::Raft(vec![0xde, 0xad, 0xbe, 0xef]));
    }

    #[test]
    fn oversized_points_count_rejected() {
        // a Write claiming a huge point count with no point data must error, not allocate
        let mut b = vec![WRITE];
        b.extend_from_slice(&1u64.to_le_bytes());
        b.extend_from_slice(&7u64.to_le_bytes());
        b.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(Message::deserialize(&b).is_err());
    }
}