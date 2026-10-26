use lattice::config::Config;
use lattice::storage::{Point, Storage};
use lattice::vector::Hnsw;
use lattice::Result;
use lattice_codec::{compress, decompress};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::time::Instant;

fn main() -> Result<()> {
    println!("=== Lattice - Load Harness ===\n");
    phase_storage()?;
    phase_codec();
    phase_vector();
    println!("done.");
    Ok(())
}

fn phase_storage() -> Result<()> {
    println!("-- Storage --");
    let dir = "./harness-data";
    let _ = std::fs::remove_dir_all(dir);

    let config = Config {
        data_dir: dir.to_string(),
        memtable_max_points: 50_000,
        ..Default::default()
    };
    let storage = Storage::open(&config)?;

    let n = 100_000usize;
    let series = 64u64;
    let start = Instant::now();
    for i in 0..n {
        storage.write(Point { series: i as u64 % series, ts: i as i64, value: i as f64 })?;
    }
    let secs = start.elapsed().as_secs_f64();
    let mb = (n * 24) as f64 / 1_048_576.0;
    println!("  write: {n} points in {secs:.3}s  ({:.0} pts/s, {:.1} MB/s)", n as f64 / secs, mb / secs);

    let start = Instant::now();
    let mut total = 0usize;
    for s in 0..series {
        total += storage.query(s, 0, n as i64)?.len();
    }
    let secs = start.elapsed().as_secs_f64();
    println!("  query: {total} points across {series} series in {secs:.3}s");

    let _ = std::fs::remove_dir_all(dir);
    println!();
    Ok(())
}

fn phase_codec() {
    println!("-- Codec (rANS) --");
    let size = 8 * 1024 * 1024;
    let mut rng = StdRng::seed_from_u64(1);
    let mut data = Vec::with_capacity(size);
    for _ in 0..size {
        let r: f64 = rng.gen();
        data.push(if r < 0.6 { 0u8 } else if r < 0.85 { 1u8 } else { rng.gen::<u8>() });
    }

    let start = Instant::now();
    let encoded = compress(&data);
    let enc = start.elapsed().as_secs_f64();

    let start = Instant::now();
    let decoded = decompress(&encoded);
    let dec = start.elapsed().as_secs_f64();

    assert_eq!(decoded, data);
    let mb = size as f64 / 1_048_576.0;
    let ratio = data.len() as f64 / encoded.len() as f64;
    println!("  encode: {:.1} MB/s | decode: {:.1} MB/s | ratio: {ratio:.2}x", mb / enc, mb / dec);
    println!();
}

fn phase_vector() {
    println!("-- Vector (HNSW) --");
    let dim = 128;
    let n = 10_000usize;
    let mut rng = StdRng::seed_from_u64(2);

    let mut index = Hnsw::new(dim);
    let start = Instant::now();
    for _ in 0..n {
        let v: Vec<f32> = (0..dim).map(|_| rng.gen::<f32>()).collect();
        index.insert(v);
    }
    let secs = start.elapsed().as_secs_f64();
    println!("  insert: {n} vectors (dim {dim}) in {secs:.3}s  ({:.0} vec/s)", n as f64 / secs);

    let queries = 1000usize;
    let start = Instant::now();
    for _ in 0..queries {
        let q: Vec<f32> = (0..dim).map(|_| rng.gen::<f32>()).collect();
        let _ = index.search(&q, 10, 64);
    }
    let secs = start.elapsed().as_secs_f64();
    println!("  search: {queries} queries in {secs:.3}s  ({:.0} q/s)", queries as f64 / secs);
    println!();
}