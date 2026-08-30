use crate::error::Result;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const FILE: &str = "MANIFEST";

pub struct Manifest {
    path: PathBuf,
    segments: Vec<String>,
}

impl Manifest {
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let path = dir.as_ref().join(FILE);
        let segments = if path.exists() {
            fs::read_to_string(&path)?
                .lines()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        } else {
            Vec::new()
        };
        Ok(Self { path, segments })
    }

    pub fn segments(&self) -> &[String] {
        &self.segments
    }

    pub fn add(&mut self, name: &str) -> Result<()> {
        self.segments.push(name.to_string());
        self.persist()
    }

    pub fn replace(&mut self, names: Vec<String>) -> Result<()> {
        self.segments = names;
        self.persist()
    }

    fn persist(&self) -> Result<()> {
        let tmp = self.path.with_extension("tmp");
        let mut f = fs::File::create(&tmp)?;
        for name in &self.segments {
            writeln!(f, "{name}")?;
        }
        f.sync_all()?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}
