pub mod column;
pub mod compaction;
pub mod gorilla;
pub mod manifest;
pub mod memtable;
pub mod segment;
pub mod wal;

use crate::config::Config;
use crate::error::Result;
use manifest::Manifest;
use memtable::MemTable;
use segment::Segment;
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;
use wal::Wal;

const COMPACT_THRESHOLD: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub series: u64,
    pub ts: i64,
    pub value: f64,
}

pub struct Storage {
    dir: PathBuf,
    max_points: usize,
    inner: Mutex<Inner>,
}

struct Inner {
    wal: Wal,
    mem: MemTable,
    manifest: Manifest,
    segments: Vec<Segment>,
    next_segment: u64,
}

impl Storage {
    pub fn open(cfg: &Config) -> Result<Self> {
        let dir = PathBuf::from(&cfg.data_dir);
        std::fs::create_dir_all(&dir)?;

        let manifest = Manifest::open(&dir)?;
        let mut segments = Vec::new();
        for name in manifest.segments() {
            segments.push(Segment::open(dir.join(name))?);
        }
        // highest id, not count: compaction shrinks the count while ids keep climbing
        let next_segment = manifest
            .segments()
            .iter()
            .filter_map(|n| parse_segment_id(n))
            .max()
            .map_or(0, |m| m + 1);

        let mut wal = Wal::open(dir.join("wal.log"))?;
        let mut mem = MemTable::new();
        for p in wal.replay()? {
            mem.insert(p);
        }

        Ok(Self {
            dir,
            max_points: cfg.memtable_max_points,
            inner: Mutex::new(Inner { wal, mem, manifest, segments, next_segment }),
        })
    }

    pub fn write(&self, p: Point) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        inner.wal.append(p)?;
        inner.mem.insert(p);
        if inner.mem.len() >= self.max_points {
            self.flush_locked(&mut inner)?;
        }
        Ok(())
    }

    pub fn write_batch(&self, points: &[Point]) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        for &p in points {
            inner.wal.append(p)?;
            inner.mem.insert(p);
        }
        if inner.mem.len() >= self.max_points {
            self.flush_locked(&mut inner)?;
        }
        Ok(())
    }

    pub fn query(&self, series: u64, start: i64, end: i64) -> Result<Vec<(i64, f64)>> {
        let inner = self.inner.lock().unwrap();
        let mut merged: BTreeMap<i64, f64> = BTreeMap::new();
        for seg in &inner.segments {
            for (ts, v) in seg.query(series, start, end) {
                merged.insert(ts, v);
            }
        }
        for (ts, v) in inner.mem.query(series, start, end) {
            merged.insert(ts, v);
        }
        Ok(merged.into_iter().collect())
    }

    pub fn stats(&self) -> StorageStats {
        let inner = self.inner.lock().unwrap();
        let mut series = HashSet::new();
        for seg in &inner.segments {
            for s in seg.series_ids() {
                series.insert(s);
            }
        }
        for s in inner.mem.series_ids() {
            series.insert(s);
        }
        StorageStats {
            series: series.len(),
            segments: inner.segments.len(),
            memtable_points: inner.mem.len(),
        }
    }

    pub fn flush(&self) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        self.flush_locked(&mut inner)
    }

    fn flush_locked(&self, inner: &mut Inner) -> Result<()> {
        if inner.mem.is_empty() {
            return Ok(());
        }
        let id = inner.next_segment;
        inner.next_segment += 1;
        let name = segment_name(id);

        let blocks = inner.mem.drain_sorted();
        Segment::build(self.dir.join(&name), &blocks)?;
        inner.segments.push(Segment::open(self.dir.join(&name))?);
        inner.manifest.add(&name)?;
        inner.wal.truncate()?;

        if inner.segments.len() >= COMPACT_THRESHOLD {
            self.compact_locked(inner)?;
        }
        Ok(())
    }

    // manifest swap is the commit point; drop old handles before deleting their files
    fn compact_locked(&self, inner: &mut Inner) -> Result<()> {
        let id = inner.next_segment;
        inner.next_segment += 1;
        let name = segment_name(id);
        let path = self.dir.join(&name);

        compaction::compact(&inner.segments, &path)?;
        inner.manifest.replace(vec![name])?;

        let old: Vec<PathBuf> = inner.segments.iter().map(|s| s.path().to_path_buf()).collect();
        inner.segments = vec![Segment::open(&path)?];
        for p in old {
            let _ = std::fs::remove_file(p);
        }
        Ok(())
    }
}

fn segment_name(id: u64) -> String {
    format!("seg-{id:08}.lsm")
}

fn parse_segment_id(name: &str) -> Option<u64> {
    name.strip_prefix("seg-")?.strip_suffix(".lsm")?.parse().ok()
}

pub struct StorageStats {
    pub series: usize,
    pub segments: usize,
    pub memtable_points: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn compaction_merges_segments() {
        let dir = tempdir().unwrap();
        let cfg = Config {
            data_dir: dir.path().to_string_lossy().into_owned(),
            memtable_max_points: 2,
            ..Default::default()
        };
        let storage = Storage::open(&cfg).unwrap();

        let flushes = COMPACT_THRESHOLD + 2;
        let n = flushes * 2;
        for i in 0..n {
            storage.write(Point { series: 1, ts: i as i64, value: i as f64 }).unwrap();
        }

        assert!(storage.stats().segments < flushes);

        let got = storage.query(1, 0, n as i64).unwrap();
        assert_eq!(got.len(), n);
        assert_eq!(got.first(), Some(&(0, 0.0)));
        assert_eq!(got.last(), Some(&(n as i64 - 1, (n - 1) as f64)));
    }

    #[test]
    fn segment_ids_survive_reopen() {
        let dir = tempdir().unwrap();
        let cfg = Config {
            data_dir: dir.path().to_string_lossy().into_owned(),
            memtable_max_points: 2,
            ..Default::default()
        };

        let half = (COMPACT_THRESHOLD + 2) * 2;

        {
            let storage = Storage::open(&cfg).unwrap();
            for i in 0..half {
                storage.write(Point { series: 1, ts: i as i64, value: i as f64 }).unwrap();
            }
        }

        // after compaction, a count-based next id would reuse a live segment on reopen and overwrite it
        let storage = Storage::open(&cfg).unwrap();
        for i in half..half * 2 {
            storage.write(Point { series: 1, ts: i as i64, value: i as f64 }).unwrap();
        }

        let n = half * 2;
        let got = storage.query(1, 0, n as i64).unwrap();
        assert_eq!(got.len(), n);
        assert_eq!(got.first(), Some(&(0, 0.0)));
        assert_eq!(got.last(), Some(&(n as i64 - 1, (n - 1) as f64)));
    }
}