use thiserror::Error;

#[derive(Error, Debug)]
pub enum LatticeError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("config: {0}")]
    Config(String),

    #[error("corrupt: {0}")]
    Corrupt(String),

    #[error("network: {0}")]
    Network(String),

    #[error("raft: {0}")]
    Raft(#[from] raft::Error),
}

pub type Result<T> = std::result::Result<T, LatticeError>;