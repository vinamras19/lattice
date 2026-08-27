use super::column;
use crate::error::{LatticeError, Result};
use memmap2::Mmap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

const MAGIC: u32 = 0x4C53_4547; // "LSEG"
const VERSION: u32 = 1;
const HEADER: usize = 12; // magic + version + count
const ENTRY: usize = 24; // series(8) + offset(8) + len(4) + count(4)

struct IndexEntry {
    series: u64,
    offset: u64,
    len: u32,
    count: u32,
}

pub struct Segment {
    mmap: Mmap,
    index: Vec<IndexEntry>,
    path: PathBuf,
}

impl Segment {
    pub fn build(path: impl AsRef<Path>, blocks: &[(u64, Vec<(i64, f64)>)]) -> Result<()> {
        let mut encoded: Vec<(u64, Vec<u8>, u32)> = Vec::with_capacity(blocks.len());
        for (series, points) in blocks {
            encoded.push((*series, column::encode(points), points.len() as u32));
        }

        let file = File::create(path)?;
        let mut w = BufWriter::new(file);
        w.write_all(&MAGIC.to_le_bytes())?;
        w.write_all(&VERSION.to_le_bytes())?;
        w.write_all(&(encoded.len() as u32).to_le_bytes())?;

        let mut offset = (HEADER + encoded.len() * ENTRY) as u64;
        for (series, data, count) in &encoded {
            w.write_all(&series.to_le_bytes())?;
            w.write_all(&offset.to_le_bytes())?;
            w.write_all(&(data.len() as u32).to_le_bytes())?;
            w.write_all(&count.to_le_bytes())?;
            offset += data.len() as u64;
        }
        for (_, data, _) in &encoded {
            w.write_all(data)?;
        }
        w.flush()?;
        Ok(())
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = File::open(&path)?;
        let mmap = unsafe { Mmap::map(&file)? };

        if mmap.len() < HEADER {
            return Err(LatticeError::Corrupt(format!("{}: short header", path.display())));
        }
        if u32::from_le_bytes(mmap[0..4].try_into().unwrap()) != MAGIC {
            return Err(LatticeError::Corrupt(format!("{}: bad magic", path.display())));
        }
        let count = u32::from_le_bytes(mmap[8..12].try_into().unwrap()) as usize;

        let mut index = Vec::with_capacity(count);
        let mut off = HEADER;
        for _ in 0..count {
            index.push(IndexEntry {
                series: u64::from_le_bytes(mmap[off..off + 8].try_into().unwrap()),
                offset: u64::from_le_bytes(mmap[off + 8..off + 16].try_into().unwrap()),
                len: u32::from_le_bytes(mmap[off + 16..off + 20].try_into().unwrap()),
                count: u32::from_le_bytes(mmap[off + 20..off + 24].try_into().unwrap()),
            });
            off += ENTRY;
        }

        Ok(Self { mmap, index, path })
    }

    pub fn query(&self, series: u64, start: i64, end: i64) -> Vec<(i64, f64)> {
        self.read_series(series)
            .into_iter()
            .filter(|&(ts, _)| ts >= start && ts < end)
            .collect()
    }

    pub fn read_series(&self, series: u64) -> Vec<(i64, f64)> {
        match self.index.binary_search_by_key(&series, |e| e.series) {
            Ok(i) => {
                let e = &self.index[i];
                let lo = e.offset as usize;
                column::decode(&self.mmap[lo..lo + e.len as usize], e.count as usize)
            }
            Err(_) => Vec::new(),
        }
    }

    pub fn series_ids(&self) -> Vec<u64> {
        self.index.iter().map(|e| e.series).collect()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}
