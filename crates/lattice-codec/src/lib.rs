mod model;
mod rans;

pub use rans::{compress, decompress};

pub const SCALE_BITS: u32 = 12;