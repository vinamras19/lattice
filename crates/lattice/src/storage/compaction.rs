use super::segment::Segment;
use crate::error::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub fn compact(segments: &[Segment], out: impl AsRef<Path>) -> Result<()> {
    let mut series: BTreeSet<u64> = BTreeSet::new();
    for seg in segments {
        series.extend(seg.series_ids());
    }

    let mut blocks: Vec<(u64, Vec<(i64, f64)>)> = Vec::with_capacity(series.len());
    for s in series {
        let mut merged: BTreeMap<i64, f64> = BTreeMap::new();
        for seg in segments {
            for (ts, v) in seg.read_series(s) {
                merged.insert(ts, v);
            }
        }
        blocks.push((s, merged.into_iter().collect()));
    }

    Segment::build(out, &blocks)
}
