use crate::model::Model;
use crate::SCALE_BITS;

const RANS_L: u32 = 1 << 23;

pub fn compress(data: &[u8]) -> Vec<u8> {
    let model = Model::from_data(data);

    let mut out = Vec::new();
    out.extend_from_slice(&(data.len() as u64).to_le_bytes());
    for f in model.freqs {
        out.extend_from_slice(&(f as u16).to_le_bytes());
    }

    let mut x = RANS_L;
    let mut payload = Vec::new();
    for &b in data.iter().rev() {
        let freq = model.freqs[b as usize];
        let start = model.cum[b as usize];
        let x_max = ((RANS_L >> SCALE_BITS) << 8) * freq;
        while x >= x_max {
            payload.push((x & 0xff) as u8);
            x >>= 8;
        }
        x = ((x / freq) << SCALE_BITS) + (x % freq) + start;
    }
    payload.extend_from_slice(&x.to_le_bytes());
    payload.reverse();

    out.extend_from_slice(&payload);
    out
}

pub fn decompress(data: &[u8]) -> Vec<u8> {
    let len = u64::from_le_bytes(data[0..8].try_into().unwrap()) as usize;

    let mut freqs = [0u32; 256];
    let mut off = 8;
    for f in &mut freqs {
        *f = u16::from_le_bytes(data[off..off + 2].try_into().unwrap()) as u32;
        off += 2;
    }
    let model = Model::from_freqs(freqs);

    let payload = &data[off..];
    let mut x = u32::from_be_bytes(payload[0..4].try_into().unwrap());
    let mut pos = 4;

    let mask = (1u32 << SCALE_BITS) - 1;
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        let slot = x & mask;
        let s = model.symbol(slot);
        let freq = model.freqs[s as usize];
        let start = model.cum[s as usize];
        x = freq * (x >> SCALE_BITS) + slot - start;
        while x < RANS_L {
            x = (x << 8) | payload[pos] as u32;
            pos += 1;
        }
        out.push(s);
    }
    out
}