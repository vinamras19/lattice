use super::Point;
use std::collections::HashMap;

pub struct MemTable {
    series: HashMap<u64, Vec<(i64, f64)>>,
    len: usize,
}

impl MemTable {
    pub fn new() -> Self {
        Self { series: HashMap::new(), len: 0 }
    }

    pub fn insert(&mut self, p: Point) {
        self.series.entry(p.series).or_default().push((p.ts, p.value));
        self.len += 1;
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn series_ids(&self) -> Vec<u64> {
        self.series.keys().copied().collect()
    }

    pub fn query(&self, series: u64, start: i64, end: i64) -> Vec<(i64, f64)> {
        let mut out: Vec<(i64, f64)> = match self.series.get(&series) {
            Some(points) => points
                .iter()
                .copied()
                .filter(|&(ts, _)| ts >= start && ts < end)
                .collect(),
            None => Vec::new(),
        };
        out.sort_by_key(|&(ts, _)| ts);
        out
    }

    pub fn drain_sorted(&mut self) -> Vec<(u64, Vec<(i64, f64)>)> {
        let mut blocks: Vec<(u64, Vec<(i64, f64)>)> = self.series.drain().collect();
        for (_, points) in &mut blocks {
            points.sort_by_key(|&(ts, _)| ts);
        }
        blocks.sort_by_key(|&(series, _)| series);
        self.len = 0;
        blocks
    }
}

impl Default for MemTable {
    fn default() -> Self {
        Self::new()
    }
}