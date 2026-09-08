use crate::{LatticeError, Result};
use protobuf::Message as _;
use raft::prelude::{Entry, EntryType, Message};
use raft::raw_node::RawNode;
use raft::storage::MemStorage;
use raft::{Config, StateRole};

pub fn discard_logger() -> slog::Logger {
    slog::Logger::root(slog::Discard, slog::o!())
}

pub fn encode_message(msg: &Message) -> Result<Vec<u8>> {
    msg.write_to_bytes()
        .map_err(|e| LatticeError::Network(format!("raft encode: {e}")))
}

pub fn decode_message(bytes: &[u8]) -> Result<Message> {
    Message::parse_from_bytes(bytes)
        .map_err(|e| LatticeError::Network(format!("raft decode: {e}")))
}

pub struct RaftNode {
    id: u64,
    raw: RawNode<MemStorage>,
    store: MemStorage,
    applied: Vec<Vec<u8>>,
}

impl RaftNode {
    pub fn new(id: u64, voters: &[u64], logger: &slog::Logger) -> Result<Self> {
        let config = Config {
            id,
            election_tick: 10,
            heartbeat_tick: 3,
            ..Default::default()
        };
        config.validate()?;

        let store = MemStorage::new_with_conf_state((voters.to_vec(), vec![]));
        let raw = RawNode::new(&config, store.clone(), logger)?;

        Ok(Self { id, raw, store, applied: Vec::new() })
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn is_leader(&self) -> bool {
        self.raw.raft.state == StateRole::Leader
    }

    pub fn tick(&mut self) -> bool {
        self.raw.tick()
    }

    pub fn campaign(&mut self) -> Result<()> {
        self.raw.campaign()?;
        Ok(())
    }

    pub fn propose(&mut self, data: Vec<u8>) -> Result<()> {
        self.raw.propose(vec![], data)?;
        Ok(())
    }

    pub fn step(&mut self, msg: Message) -> Result<()> {
        self.raw.step(msg)?;
        Ok(())
    }

    pub fn applied(&self) -> &[Vec<u8>] {
        &self.applied
    }

    // process one Ready: persist state, collect outbound messages, apply committed entries
    pub fn process_ready(&mut self) -> Vec<Message> {
        if !self.raw.has_ready() {
            return Vec::new();
        }

        let mut out = Vec::new();
        let mut ready = self.raw.ready();

        out.append(&mut ready.take_messages());

        if !ready.snapshot().is_empty() {
            let snapshot = ready.snapshot().clone();
            self.store.wl().apply_snapshot(snapshot).unwrap();
        }

        self.apply(ready.take_committed_entries());

        if !ready.entries().is_empty() {
            self.store.wl().append(ready.entries()).unwrap();
        }

        if let Some(hs) = ready.hs() {
            self.store.wl().set_hardstate(hs.clone());
        }

        out.append(&mut ready.take_persisted_messages());

        let mut light = self.raw.advance(ready);
        if let Some(commit) = light.commit_index() {
            self.store.wl().mut_hard_state().set_commit(commit);
        }
        out.append(&mut light.take_messages());
        self.apply(light.take_committed_entries());
        self.raw.advance_apply();

        out
    }

    fn apply(&mut self, entries: Vec<Entry>) {
        for entry in entries {
            if entry.get_data().is_empty() {
                continue; // empty entry appended on leadership change
            }
            if entry.get_entry_type() == EntryType::EntryNormal {
                self.applied.push(entry.get_data().to_vec());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const MSG: &[u8] = b"hello-raft";

    #[test]
    fn elects_leader_and_replicates() {
        let logger = discard_logger();
        let ids = [1u64, 2, 3];

        let mut nodes: BTreeMap<u64, RaftNode> = BTreeMap::new();
        for &id in &ids {
            nodes.insert(id, RaftNode::new(id, &ids, &logger).unwrap());
        }

        // force node 1 to start an election rather than wait on timeouts
        nodes.get_mut(&1).unwrap().campaign().unwrap();

        let mut proposed = false;
        let mut committed_everywhere = false;

        for _ in 0..200 {
            for node in nodes.values_mut() {
                node.tick();
            }

            let mut outbox: Vec<Message> = Vec::new();
            for node in nodes.values_mut() {
                outbox.append(&mut node.process_ready());
            }
            for msg in outbox {
                if let Some(node) = nodes.get_mut(&msg.get_to()) {
                    let _ = node.step(msg);
                }
            }

            if !proposed {
                if let Some(leader) = nodes.values_mut().find(|n| n.is_leader()) {
                    leader.propose(MSG.to_vec()).unwrap();
                    proposed = true;
                }
            }

            if proposed
                && nodes
                    .values()
                    .all(|n| n.applied().iter().any(|d| d.as_slice() == MSG))
            {
                committed_everywhere = true;
                break;
            }
        }

        assert!(nodes.values().any(|n| n.is_leader()), "no leader elected");
        assert!(committed_everywhere, "entry not replicated to all nodes");
    }

    #[test]
    fn raft_message_roundtrips_through_bytes() {
        use raft::prelude::MessageType;

        let mut m = Message::default();
        m.set_msg_type(MessageType::MsgRequestVote);
        m.set_to(2);
        m.set_from(1);
        m.set_term(5);

        let bytes = encode_message(&m).unwrap();
        let back = decode_message(&bytes).unwrap();

        assert_eq!(back, m);
    }
}