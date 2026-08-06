use super::Point;
use crate::error::Result;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

const RECORD: usize = 24; // series(8) + ts(8) + value(8)

pub struct Wal {
    path: PathBuf,
    writer: BufWriter<File>,
}

impl Wal {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        Ok(Self { path, writer: BufWriter::new(file) })
    }

    pub fn append(&mut self, p: Point) -> Result<()> {
        self.writer.write_all(&p.series.to_le_bytes())?;
        self.writer.write_all(&p.ts.to_le_bytes())?;
        self.writer.write_all(&p.value.to_bits().to_le_bytes())?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn replay(&mut self) -> Result<Vec<Point>> {
        let mut bytes = Vec::new();
        File::open(&self.path)?.read_to_end(&mut bytes)?;

        let mut out = Vec::with_capacity(bytes.len() / RECORD);
        for chunk in bytes.chunks_exact(RECORD) {
            out.push(Point {
                series: u64::from_le_bytes(chunk[0..8].try_into().unwrap()),
                ts: i64::from_le_bytes(chunk[8..16].try_into().unwrap()),
                value: f64::from_bits(u64::from_le_bytes(chunk[16..24].try_into().unwrap())),
            });
        }
        Ok(out)
    }

    pub fn truncate(&mut self) -> Result<()> {
        self.writer.flush()?;
        OpenOptions::new().write(true).open(&self.path)?.set_len(0)?;
        let file = OpenOptions::new().create(true).append(true).open(&self.path)?;
        self.writer = BufWriter::new(file);
        Ok(())
    }
}
