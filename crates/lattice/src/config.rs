use crate::error::{LatticeError, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub node_id: u64,
    pub data_dir: String,
    pub listen_addr: String,
    pub api_addr: String,
    pub memtable_max_points: usize,
    pub peers: Vec<RaftPeer>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RaftPeer {
    pub id: u64,
    pub addr: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            node_id: 1,
            data_dir: "./data".into(),
            listen_addr: "0.0.0.0:7700".into(),
            api_addr: "0.0.0.0:7800".into(),
            memtable_max_points: 100_000,
            peers: Vec::new(),
        }
    }
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)?;
        toml::from_str(&text).map_err(|e| LatticeError::Config(e.to_string()))
    }
}