use crate::SCALE_BITS;

pub struct Model {
    pub freqs: [u32; 256],
    pub cum: [u32; 257],
    slot2sym: Vec<u8>,
}

impl Model {
    pub fn from_data(data: &[u8]) -> Self {
        let mut counts = [0u32; 256];
        for &b in data {
            counts[b as usize] += 1;
        }
        Self::from_freqs(normalize(&counts))
    }

    pub fn from_freqs(freqs: [u32; 256]) -> Self {
        let mut cum = [0u32; 257];
        for i in 0..256 {
            cum[i + 1] = cum[i] + freqs[i];
        }

        let mut slot2sym = vec![0u8; cum[256] as usize];
        for s in 0..256 {
            slot2sym[cum[s] as usize..cum[s + 1] as usize].fill(s as u8);
        }

        Self { freqs, cum, slot2sym }
    }

    pub fn symbol(&self, slot: u32) -> u8 {
        self.slot2sym[slot as usize]
    }
}

fn normalize(counts: &[u32; 256]) -> [u32; 256] {
    let total: u64 = counts.iter().map(|&c| c as u64).sum();
    let target = 1u32 << SCALE_BITS;
    let mut freqs = [0u32; 256];
    if total == 0 {
        return freqs;
    }

    let mut sum = 0u32;
    for i in 0..256 {
        if counts[i] == 0 {
            continue;
        }
        let mut f = (counts[i] as u64 * target as u64 / total) as u32;
        if f == 0 {
            f = 1;
        }
        freqs[i] = f;
        sum += f;
    }

    // absorb rounding drift into the most frequent symbol
    if sum != target {
        let mut max = 0usize;
        for i in 1..256 {
            if freqs[i] > freqs[max] {
                max = i;
            }
        }
        freqs[max] = (freqs[max] as i64 + (target as i64 - sum as i64)) as u32;
    }

    freqs
}