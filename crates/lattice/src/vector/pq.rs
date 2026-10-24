use super::distance::l2;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

const K: usize = 256; // centroids per subspace -> one byte per code
const KMEANS_ITERS: usize = 25;
const SEED: u64 = 0x9d;

pub struct ProductQuantizer {
    dim: usize,
    m: usize,
    sub_dim: usize,
    codebooks: Vec<Vec<f32>>, // codebooks[subspace] = K * sub_dim flat
}

impl ProductQuantizer {
    pub fn train(vectors: &[Vec<f32>], dim: usize, m: usize) -> Self {
        assert!(!vectors.is_empty(), "training set is empty");
        assert!(dim.is_multiple_of(m), "dim must be divisible by m");
        let sub_dim = dim / m;
        let mut rng = StdRng::seed_from_u64(SEED);

        let mut codebooks = Vec::with_capacity(m);
        for s in 0..m {
            let lo = s * sub_dim;
            let subvectors: Vec<&[f32]> = vectors.iter().map(|v| &v[lo..lo + sub_dim]).collect();
            codebooks.push(kmeans(&subvectors, sub_dim, &mut rng));
        }

        Self { dim, m, sub_dim, codebooks }
    }

    pub fn encode(&self, vector: &[f32]) -> Vec<u8> {
        debug_assert_eq!(vector.len(), self.dim);
        let mut codes = Vec::with_capacity(self.m);
        for s in 0..self.m {
            let lo = s * self.sub_dim;
            codes.push(self.nearest(s, &vector[lo..lo + self.sub_dim]) as u8);
        }
        codes
    }

    pub fn decode(&self, codes: &[u8]) -> Vec<f32> {
        debug_assert_eq!(codes.len(), self.m);
        let mut v = Vec::with_capacity(self.dim);
        for (s, &code) in codes.iter().enumerate() {
            v.extend_from_slice(self.centroid(s, code as usize));
        }
        v
    }

    pub fn distance_table(&self, query: &[f32]) -> Vec<f32> {
        debug_assert_eq!(query.len(), self.dim);
        let mut table = vec![0f32; self.m * K];
        for s in 0..self.m {
            let lo = s * self.sub_dim;
            let q = &query[lo..lo + self.sub_dim];
            for k in 0..K {
                table[s * K + k] = l2(q, self.centroid(s, k));
            }
        }
        table
    }

    pub fn distance(&self, table: &[f32], codes: &[u8]) -> f32 {
        let mut sum = 0f32;
        for s in 0..self.m {
            sum += table[s * K + codes[s] as usize];
        }
        sum
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    fn centroid(&self, subspace: usize, k: usize) -> &[f32] {
        let start = k * self.sub_dim;
        &self.codebooks[subspace][start..start + self.sub_dim]
    }

    fn nearest(&self, subspace: usize, sub: &[f32]) -> usize {
        let mut best = 0;
        let mut best_d = f32::INFINITY;
        for k in 0..K {
            let d = l2(sub, self.centroid(subspace, k));
            if d < best_d {
                best_d = d;
                best = k;
            }
        }
        best
    }
}

fn kmeans(points: &[&[f32]], dim: usize, rng: &mut StdRng) -> Vec<f32> {
    let n = points.len();
    let mut centroids = vec![0f32; K * dim];
    for c in 0..K {
        let src = points[rng.gen_range(0..n)];
        centroids[c * dim..(c + 1) * dim].copy_from_slice(src);
    }

    let mut assign = vec![0usize; n];
    for _ in 0..KMEANS_ITERS {
        let mut changed = false;
        for (i, p) in points.iter().enumerate() {
            let mut best = 0;
            let mut best_d = f32::INFINITY;
            for c in 0..K {
                let d = l2(p, &centroids[c * dim..(c + 1) * dim]);
                if d < best_d {
                    best_d = d;
                    best = c;
                }
            }
            if assign[i] != best {
                assign[i] = best;
                changed = true;
            }
        }

        let mut sums = vec![0f32; K * dim];
        let mut counts = vec![0usize; K];
        for (i, p) in points.iter().enumerate() {
            let c = assign[i];
            counts[c] += 1;
            for d in 0..dim {
                sums[c * dim + d] += p[d];
            }
        }
        for c in 0..K {
            if counts[c] > 0 {
                for d in 0..dim {
                    centroids[c * dim + d] = sums[c * dim + d] / counts[c] as f32;
                }
            }
        }

        if !changed {
            break;
        }
    }
    centroids
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vector::distance::l2;
    use std::collections::HashSet;

    #[test]
    fn pq_adc_recall() {
        let dim = 64;
        let m = 8;
        let n = 2000;
        let clusters = 50;
        let mut rng = StdRng::seed_from_u64(11);

        let centers: Vec<Vec<f32>> = (0..clusters)
            .map(|_| (0..dim).map(|_| rng.gen::<f32>() * 10.0).collect())
            .collect();
        let sample = |rng: &mut StdRng| -> Vec<f32> {
            let c = &centers[rng.gen_range(0..clusters)];
            c.iter().map(|&x| x + rng.gen::<f32>()).collect()
        };

        let vectors: Vec<Vec<f32>> = (0..n).map(|_| sample(&mut rng)).collect();
        let pq = ProductQuantizer::train(&vectors, dim, m);
        let codes: Vec<Vec<u8>> = vectors.iter().map(|v| pq.encode(v)).collect();

        let k = 10;
        let mut hits = 0;
        let mut total = 0;
        for _ in 0..50 {
            let q = sample(&mut rng);

            let mut exact: Vec<(f32, usize)> =
                vectors.iter().enumerate().map(|(i, v)| (l2(&q, v), i)).collect();
            exact.sort_by(|a, b| a.0.total_cmp(&b.0));
            let truth: HashSet<usize> = exact.iter().take(k).map(|&(_, i)| i).collect();

            let table = pq.distance_table(&q);
            let mut approx: Vec<(f32, usize)> =
                codes.iter().enumerate().map(|(i, c)| (pq.distance(&table, c), i)).collect();
            approx.sort_by(|a, b| a.0.total_cmp(&b.0));

            for &(_, i) in approx.iter().take(k) {
                if truth.contains(&i) {
                    hits += 1;
                }
                total += 1;
            }
        }

        let recall = hits as f64 / total as f64;
        assert!(recall > 0.35, "pq recall too low: {recall}");
    }
}