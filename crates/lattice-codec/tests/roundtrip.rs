use lattice_codec::{compress, decompress};
use proptest::prelude::*;

proptest! {
    #[test]
    fn roundtrip(data: Vec<u8>) {
        prop_assert_eq!(decompress(&compress(&data)), data);
    }
}

#[test]
fn roundtrip_empty() {
    assert_eq!(decompress(&compress(&[])), Vec::<u8>::new());
}

#[test]
fn roundtrip_single_symbol() {
    let data = vec![7u8; 5000];
    assert_eq!(decompress(&compress(&data)), data);
}

#[test]
fn roundtrip_all_bytes() {
    let data: Vec<u8> = (0..=255).cycle().take(10_000).collect();
    assert_eq!(decompress(&compress(&data)), data);
}
