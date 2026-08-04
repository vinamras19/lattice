struct BitWriter {
    buf: Vec<u8>,
    cur: u8,
    nbits: u8,
}

impl BitWriter {
    fn new() -> Self {
        Self { buf: Vec::new(), cur: 0, nbits: 0 }
    }

    fn write_bit(&mut self, bit: u32) {
        self.cur = (self.cur << 1) | (bit as u8 & 1);
        self.nbits += 1;
        if self.nbits == 8 {
            self.buf.push(self.cur);
            self.cur = 0;
            self.nbits = 0;
        }
    }

    fn write_bits(&mut self, val: u64, n: u32) {
        for i in (0..n).rev() {
            self.write_bit(((val >> i) & 1) as u32);
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.nbits > 0 {
            self.cur <<= 8 - self.nbits;
            self.buf.push(self.cur);
        }
        self.buf
    }
}

struct BitReader<'a> {
    buf: &'a [u8],
    byte: usize,
    bit: u8,
}

impl<'a> BitReader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, byte: 0, bit: 0 }
    }

    fn read_bit(&mut self) -> u32 {
        let b = (self.buf[self.byte] >> (7 - self.bit)) & 1;
        self.bit += 1;
        if self.bit == 8 {
            self.bit = 0;
            self.byte += 1;
        }
        b as u32
    }

    fn read_bits(&mut self, n: u32) -> u64 {
        let mut v = 0u64;
        for _ in 0..n {
            v = (v << 1) | self.read_bit() as u64;
        }
        v
    }
}

fn write_signed(w: &mut BitWriter, val: i64, n: u32) {
    w.write_bits(val as u64, n);
}

fn read_signed(r: &mut BitReader, n: u32) -> i64 {
    let raw = r.read_bits(n);
    // sign-extend from n bits
    ((raw << (64 - n)) as i64) >> (64 - n)
}

pub fn encode(points: &[(i64, f64)]) -> Vec<u8> {
    let mut w = BitWriter::new();
    if points.is_empty() {
        return w.finish();
    }

    let (mut prev_ts, first) = points[0];
    w.write_bits(prev_ts as u64, 64);
    let mut prev_bits = first.to_bits();
    w.write_bits(prev_bits, 64);

    let mut prev_delta: i64 = 0;
    let mut prev_lead: u32 = 64; // no prior window yet
    let mut prev_trail: u32 = 0;

    for &(ts, val) in &points[1..] {
        let delta = ts - prev_ts;
        let dod = delta - prev_delta;
        if dod == 0 {
            w.write_bit(0);
        } else if (-64..=63).contains(&dod) {
            w.write_bits(0b10, 2);
            write_signed(&mut w, dod, 7);
        } else if (-256..=255).contains(&dod) {
            w.write_bits(0b110, 3);
            write_signed(&mut w, dod, 9);
        } else if (-2048..=2047).contains(&dod) {
            w.write_bits(0b1110, 4);
            write_signed(&mut w, dod, 12);
        } else {
            w.write_bits(0b1111, 4);
            write_signed(&mut w, dod, 64);
        }
        prev_ts = ts;
        prev_delta = delta;

        let bits = val.to_bits();
        let xor = bits ^ prev_bits;
        if xor == 0 {
            w.write_bit(0);
        } else {
            w.write_bit(1);
            let mut lead = xor.leading_zeros();
            let trail = xor.trailing_zeros();
            if lead > 31 {
                lead = 31;
            }
            if prev_lead != 64 && lead >= prev_lead && trail >= prev_trail {
                // reuse the previous leading/trailing window
                w.write_bit(0);
                let meaningful = 64 - prev_lead - prev_trail;
                w.write_bits(xor >> prev_trail, meaningful);
            } else {
                w.write_bit(1);
                w.write_bits(lead as u64, 5);
                w.write_bits(trail as u64, 6);
                let meaningful = 64 - lead - trail;
                w.write_bits(xor >> trail, meaningful);
                prev_lead = lead;
                prev_trail = trail;
            }
        }
        prev_bits = bits;
    }

    w.finish()
}

pub fn decode(data: &[u8], count: usize) -> Vec<(i64, f64)> {
    let mut out = Vec::with_capacity(count);
    if count == 0 {
        return out;
    }

    let mut r = BitReader::new(data);
    let mut ts = r.read_bits(64) as i64;
    let mut bits = r.read_bits(64);
    out.push((ts, f64::from_bits(bits)));

    let mut prev_delta: i64 = 0;
    let mut prev_lead: u32 = 64;
    let mut prev_trail: u32 = 0;

    for _ in 1..count {
        let dod = if r.read_bit() == 0 {
            0
        } else if r.read_bit() == 0 {
            read_signed(&mut r, 7)
        } else if r.read_bit() == 0 {
            read_signed(&mut r, 9)
        } else if r.read_bit() == 0 {
            read_signed(&mut r, 12)
        } else {
            read_signed(&mut r, 64)
        };
        let delta = prev_delta + dod;
        ts += delta;
        prev_delta = delta;

        if r.read_bit() == 1 {
            if r.read_bit() == 0 {
                let meaningful = 64 - prev_lead - prev_trail;
                bits ^= r.read_bits(meaningful) << prev_trail;
            } else {
                let lead = r.read_bits(5) as u32;
                let trail = r.read_bits(6) as u32;
                let meaningful = 64 - lead - trail;
                bits ^= r.read_bits(meaningful) << trail;
                prev_lead = lead;
                prev_trail = trail;
            }
        }
        out.push((ts, f64::from_bits(bits)));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(points: Vec<(i64, f64)>) {
        let encoded = encode(&points);
        assert_eq!(decode(&encoded, points.len()), points);
    }

    #[test]
    fn empty() {
        assert_eq!(decode(&encode(&[]), 0), Vec::new());
    }

    #[test]
    fn regular_series() {
        let points: Vec<(i64, f64)> = (0..1000)
            .map(|i| (1_700_000_000 + i * 10, 42.0 + (i as f64) * 0.5))
            .collect();
        check(points);
    }

    #[test]
    fn irregular_and_constant() {
        check(vec![
            (100, 1.0),
            (137, 1.0),
            (140, 1.0),
            (300, -9.5),
            (305, f64::MAX),
            (306, 0.0),
        ]);
    }

    #[test]
    fn large_timestamp_jumps() {
        check(vec![
            (0, 1.0),
            (10, 2.0),
            (1_000_000_000, 3.0),
            (1_000_000_001, 4.0),
            (500, 5.0),
        ]);
    }
}