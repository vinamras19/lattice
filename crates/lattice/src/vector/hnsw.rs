use super::distance::l2;
use super::pq::ProductQuantizer;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashSet};

const M: usize = 16;
const EF_CONSTRUCTION: usize = 200;
const SEED: u64 = 0x5eed;

#[derive(Clone, Copy)]
struct Candidate {
    dist: f32,
    id: usize,
}

impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.dist == other.dist
    }
}
impl Eq for Candidate {}
impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.dist.total_cmp(&other.dist)
    }
}

// Exact stores full vectors and uses L2. Pq stores one byte per subspace and
// scores via ADC; node-to-node distances reconstruct the base from its code.
enum Backing {
    Exact { vectors: Vec<Vec<f32>> },
    Pq { pq: ProductQuantizer, codes: Vec<Vec<u8>> },
}

pub struct Hnsw {
    dim: usize,
    m: usize,
    m0: usize,
    ef_construction: usize,
    ml: f64,
    backing: Backing,
    links: Vec<Vec<Vec<usize>>>,
    entry: Option<usize>,
    max_level: usize,
    rng: StdRng,
}

impl Hnsw {
    pub fn new(dim: usize) -> Self {
        Self::with_backing(dim, Backing::Exact { vectors: Vec::new() })
    }

    pub fn with_pq(pq: ProductQuantizer) -> Self {
        let dim = pq.dim();
        Self::with_backing(dim, Backing::Pq { pq, codes: Vec::new() })
    }

    fn with_backing(dim: usize, backing: Backing) -> Self {
        Self {
            dim,
            m: M,
            m0: M * 2,
            ef_construction: EF_CONSTRUCTION,
            ml: 1.0 / (M as f64).ln(),
            backing,
            links: Vec::new(),
            entry: None,
            max_level: 0,
            rng: StdRng::seed_from_u64(SEED),
        }
    }

    pub fn len(&self) -> usize {
        self.links.len()
    }

    pub fn is_empty(&self) -> bool {
        self.links.is_empty()
    }

    pub fn insert(&mut self, vector: Vec<f32>) -> usize {
        debug_assert_eq!(vector.len(), self.dim);
        let id = self.links.len();
        let level = self.gen_level();
        let ctx = self.ctx_query(&vector);
        self.add(vector);
        self.links.push(vec![Vec::new(); level + 1]);

        let entry = match self.entry {
            None => {
                self.entry = Some(id);
                self.max_level = level;
                return id;
            }
            Some(e) => e,
        };

        let mut ep = entry;
        let mut l = self.max_level;
        while l > level {
            if let Some(c) = self.search_layer(&ctx, ep, 1, l).first() {
                ep = c.id;
            }
            l -= 1;
        }

        let start = level.min(self.max_level);
        for l in (0..=start).rev() {
            let candidates = self.search_layer(&ctx, ep, self.ef_construction, l);
            let m = if l == 0 { self.m0 } else { self.m };

            let selected: Vec<usize> = candidates.iter().take(m).map(|c| c.id).collect();
            self.links[id][l] = selected.clone();

            for &nb in &selected {
                self.links[nb][l].push(id);
                if self.links[nb][l].len() > m {
                    self.prune(nb, l, m);
                }
            }

            if let Some(c) = candidates.first() {
                ep = c.id;
            }
        }

        if level > self.max_level {
            self.max_level = level;
            self.entry = Some(id);
        }
        id
    }

    pub fn search(&self, query: &[f32], k: usize, ef: usize) -> Vec<(usize, f32)> {
        let entry = match self.entry {
            Some(e) => e,
            None => return Vec::new(),
        };

        let ctx = self.ctx_query(query);
        let mut ep = entry;
        let mut l = self.max_level;
        while l > 0 {
            if let Some(c) = self.search_layer(&ctx, ep, 1, l).first() {
                ep = c.id;
            }
            l -= 1;
        }

        let mut results = self.search_layer(&ctx, ep, ef.max(k), 0);
        results.truncate(k);
        results.into_iter().map(|c| (c.id, c.dist)).collect()
    }

    // Distance context for a query or an incoming vector: the vector itself in
    // exact mode, a precomputed ADC table in PQ mode.
    fn ctx_query(&self, q: &[f32]) -> Vec<f32> {
        match &self.backing {
            Backing::Exact { .. } => q.to_vec(),
            Backing::Pq { pq, .. } => pq.distance_table(q),
        }
    }

    // Distance context for an already-stored node, used when pruning its edges.
    fn ctx_node(&self, id: usize) -> Vec<f32> {
        match &self.backing {
            Backing::Exact { vectors } => vectors[id].clone(),
            Backing::Pq { pq, codes } => pq.distance_table(&pq.decode(&codes[id])),
        }
    }

    fn dist(&self, ctx: &[f32], id: usize) -> f32 {
        match &self.backing {
            Backing::Exact { vectors } => l2(ctx, &vectors[id]),
            Backing::Pq { pq, codes } => pq.distance(ctx, &codes[id]),
        }
    }

    fn add(&mut self, vector: Vec<f32>) {
        match &mut self.backing {
            Backing::Exact { vectors } => vectors.push(vector),
            Backing::Pq { pq, codes } => codes.push(pq.encode(&vector)),
        }
    }

    fn search_layer(&self, ctx: &[f32], entry: usize, ef: usize, layer: usize) -> Vec<Candidate> {
        let mut visited = HashSet::new();
        let mut candidates = BinaryHeap::new();
        let mut results = BinaryHeap::new();

        let d0 = self.dist(ctx, entry);
        visited.insert(entry);
        candidates.push(Reverse(Candidate { dist: d0, id: entry }));
        results.push(Candidate { dist: d0, id: entry });

        while let Some(Reverse(c)) = candidates.pop() {
            let farthest = results.peek().map(|x| x.dist).unwrap_or(f32::INFINITY);
            if c.dist > farthest {
                break;
            }
            if layer >= self.links[c.id].len() {
                continue;
            }
            for &nb in &self.links[c.id][layer] {
                if visited.insert(nb) {
                    let d = self.dist(ctx, nb);
                    let farthest = results.peek().map(|x| x.dist).unwrap_or(f32::INFINITY);
                    if d < farthest || results.len() < ef {
                        candidates.push(Reverse(Candidate { dist: d, id: nb }));
                        results.push(Candidate { dist: d, id: nb });
                        if results.len() > ef {
                            results.pop();
                        }
                    }
                }
            }
        }

        let mut out = results.into_vec();
        out.sort();
        out
    }

    fn prune(&mut self, node: usize, layer: usize, m: usize) {
        let base = self.ctx_node(node);
        let mut cand: Vec<Candidate> = self.links[node][layer]
            .iter()
            .map(|&x| Candidate { dist: self.dist(&base, x), id: x })
            .collect();
        cand.sort();
        cand.truncate(m);
        self.links[node][layer] = cand.into_iter().map(|c| c.id).collect();
    }

    fn gen_level(&mut self) -> usize {
        let r: f64 = self.rng.gen::<f64>().max(1e-12);
        (-r.ln() * self.ml) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vector::distance::l2;
    use crate::vector::pq::ProductQuantizer;

    fn brute_force(vectors: &[Vec<f32>], q: &[f32], k: usize) -> Vec<usize> {
        let mut d: Vec<Candidate> = vectors
            .iter()
            .enumerate()
            .map(|(i, v)| Candidate { dist: l2(q, v), id: i })
            .collect();
        d.sort();
        d.into_iter().take(k).map(|c| c.id).collect()
    }

    fn clustered(n: usize, dim: usize, clusters: usize, seed: u64) -> Vec<Vec<f32>> {
        let mut rng = StdRng::seed_from_u64(seed);
        let centers: Vec<Vec<f32>> = (0..clusters)
            .map(|_| (0..dim).map(|_| rng.gen::<f32>() * 10.0).collect())
            .collect();
        (0..n)
            .map(|_| {
                let c = &centers[rng.gen_range(0..clusters)];
                c.iter().map(|&x| x + rng.gen::<f32>()).collect()
            })
            .collect()
    }

    #[test]
    fn recall_against_brute_force() {
        let dim = 16;
        let n = 1000;
        let mut rng = StdRng::seed_from_u64(7);
        let vectors: Vec<Vec<f32>> = (0..n)
            .map(|_| (0..dim).map(|_| rng.gen::<f32>()).collect())
            .collect();

        let mut index = Hnsw::new(dim);
        for v in &vectors {
            index.insert(v.clone());
        }

        let k = 10;
        let mut hits = 0;
        let mut total = 0;
        for _ in 0..50 {
            let q: Vec<f32> = (0..dim).map(|_| rng.gen::<f32>()).collect();
            let truth: HashSet<usize> = brute_force(&vectors, &q, k).into_iter().collect();
            for (id, _) in index.search(&q, k, 64) {
                if truth.contains(&id) {
                    hits += 1;
                }
                total += 1;
            }
        }

        let recall = hits as f64 / total as f64;
        assert!(recall > 0.8, "recall too low: {recall}");
    }

    #[test]
    fn pq_recall_against_brute_force() {
        let dim = 32;
        let m = 8;
        let n = 600;
        let vectors = clustered(n, dim, 30, 13);

        let pq = ProductQuantizer::train(&vectors, dim, m);
        let mut index = Hnsw::with_pq(pq);
        for v in &vectors {
            index.insert(v.clone());
        }

        let k = 10;
        let mut rng = StdRng::seed_from_u64(29);
        let mut hits = 0;
        let mut total = 0;
        for _ in 0..50 {
            let base = &vectors[rng.gen_range(0..n)];
            let q: Vec<f32> = base.iter().map(|&x| x + rng.gen::<f32>()).collect();
            let truth: HashSet<usize> = brute_force(&vectors, &q, k).into_iter().collect();
            for (id, _) in index.search(&q, k, 64) {
                if truth.contains(&id) {
                    hits += 1;
                }
                total += 1;
            }
        }

        let recall = hits as f64 / total as f64;
        assert!(recall > 0.30, "pq-hnsw recall too low: {recall}");
    }
}