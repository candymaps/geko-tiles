//! Measures the service without the host: loads a part of the tiles and asks for tiles of the scope
//! from several threads at once, as `handle` is called by the host:
//!
//!   cargo run --release -p geko-module-tiles --example serve -- <bundle> <bundle.stamp.json> [threads] [requests per thread]
//!
//! `<bundle>` holds the part under `tiles/`; the meta is the `info` of the stamp of the `bundle`
//! stage. The tiles asked are random, with every zoom from 0 to 14 as likely, and inside the boxes
//! of the scope; it prints the latency (p50, p99, p99.9) and the throughput.

use std::time::Instant;

use geko_module_sdk::bundle::{BundleManifest, BundleModule};
use geko_module_sdk::module::{BundleView, CONTRACT, Context, DataPolicy, ModuleName, Request};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, stamp, rest @ ..] = args.as_slice() else {
        return Err("usage: serve <bundle> <bundle.stamp.json> [threads] [requests per thread]".into());
    };
    let threads: usize = rest.first().map_or(Ok(16), |t| t.parse())?;
    let per_thread: usize = rest.get(1).map_or(Ok(200_000), |n| n.parse())?;
    let stamp: serde_json::Value = serde_json::from_slice(&std::fs::read(stamp)?)?;
    let meta = stamp["info"].clone();
    let boxes: Vec<[f64; 4]> = meta["scope"].as_array().ok_or("no scope in the meta")?.iter().map(|b| ["west", "south", "east", "north"].map(|k| b[k].as_f64().unwrap_or(0.0))).collect();

    let module = geko_tiles(&meta, root)?;
    println!("loaded in {} ms, RSS {} MB", module.1, rss_mb());
    let module = module.0;
    // A style, a font and a sprite, as a map asks for them.
    for (route, params) in [
        ("styles/{flavor}/{lang}", vec![("flavor", "light"), ("lang", "es.json")]),
        ("fonts/{fontstack}/{range}", vec![("fontstack", "Noto Sans Regular"), ("range", "0-255.pbf")]),
        ("sprites/{name}", vec![("name", "light@2x.png")]),
    ] {
        let params = params.into_iter().map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
        let r = module.handle(Request { route: route.into(), params, query: vec![("v".into(), "bench".into())], ..Default::default() });
        let encoding = r.headers.iter().find(|(k, _)| k == "Content-Encoding").map_or("identity", |(_, v)| v.as_str());
        println!("{route}: {} {} {} bytes ({encoding})", r.status, r.content_type, r.body.len());
    }

    // Ranges of tiles of each zoom that the boxes touch.
    let ranges: Vec<Vec<(u32, u32, u32, u32)>> = (0..=14u8).map(|z| boxes.iter().map(|b| tiles(b, z)).collect()).collect();
    let run = |threads: usize, per_thread: usize| {
        let started = Instant::now();
        let mut latencies: Vec<u64> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..threads)
                .map(|t| {
                    let (module, ranges) = (&module, &ranges);
                    s.spawn(move || {
                        let mut rng = 0x9e37_79b9_7f4a_7c15u64 ^ (t as u64 + 1).wrapping_mul(0xbf58_476d_1ce4_e5b9);
                        let mut out = Vec::with_capacity(per_thread);
                        for _ in 0..per_thread {
                            let z = (next(&mut rng) % 15) as usize;
                            let r = ranges[z][(next(&mut rng) % ranges[z].len() as u64) as usize];
                            let x = r.0 + (next(&mut rng) % u64::from(r.2 - r.0 + 1)) as u32;
                            let y = r.1 + (next(&mut rng) % u64::from(r.3 - r.1 + 1)) as u32;
                            let request = Request {
                                route: "{z}/{x}/{y}".into(),
                                params: vec![("z".into(), z.to_string()), ("x".into(), x.to_string()), ("y".into(), y.to_string())],
                                query: vec![("v".into(), "bench".into())],
                                ..Default::default()
                            };
                            let at = Instant::now();
                            let response = module.handle(request);
                            out.push(at.elapsed().as_nanos() as u64);
                            assert!(matches!(response.status, 200 | 204), "{z}/{x}/{y}: {}", response.status);
                        }
                        out
                    })
                })
                .collect();
            handles.into_iter().flat_map(|h| h.join().expect("a thread")).collect()
        });
        let elapsed = started.elapsed();
        latencies.sort_unstable();
        let at = |q: f64| latencies[((latencies.len() - 1) as f64 * q) as usize] as f64 / 1000.0;
        println!(
            "{threads:>2} threads: {:>10.0} req/s  p50 {:.1} µs  p99 {:.1} µs  p99.9 {:.1} µs",
            latencies.len() as f64 / elapsed.as_secs_f64(),
            at(0.5),
            at(0.99),
            at(0.999)
        );
    };
    run(threads, per_thread); // warms the page cache
    run(1, per_thread);
    run(threads, per_thread);
    let mut metrics = String::new();
    module.metrics(&mut metrics);
    print!("{}", metrics.lines().filter(|l| !l.starts_with('#')).map(|l| format!("{l}\n")).collect::<String>());
    println!("RSS {} MB", rss_mb());
    Ok(())
}

/// The module, with the part of `root` loaded, and how long the load took in ms.
fn geko_tiles(meta: &serde_json::Value, root: &str) -> Result<(Box<dyn geko_module_sdk::module::Module>, u128), Box<dyn std::error::Error>> {
    let data = tempfile_dir();
    let module = geko_module_tiles::open(Context { config: String::new(), config_file: String::new(), scope: Default::default(), data_dir: data, data: DataPolicy::default() })?;
    let entry = BundleModule { name: ModuleName::new("tiles")?, version: "0".into(), contract: CONTRACT, meta: meta.clone() };
    let manifest = BundleManifest { format: 1, version: "bench".into(), created_unix: 0, modules: vec![entry.clone()], licenses: vec![], artifacts: vec![] };
    let started = Instant::now();
    module.load(&BundleView { root: root.into(), manifest, module: entry })?;
    Ok((module, started.elapsed().as_millis()))
}

fn tempfile_dir() -> std::path::PathBuf {
    std::env::temp_dir().join("geko-tiles-serve")
}

fn tiles(b: &[f64; 4], z: u8) -> (u32, u32, u32, u32) {
    let n = f64::from(1u32 << z);
    let x = |lon: f64| (((lon + 180.0) / 360.0 * n).floor().clamp(0.0, n - 1.0)) as u32;
    let y = |lat: f64| {
        let lat = lat.to_radians();
        (((1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0 * n).floor().clamp(0.0, n - 1.0)) as u32
    };
    (x(b[0]), y(b[3]), x(b[2]), y(b[1]))
}

/// splitmix64.
fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn rss_mb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok()))
        .map_or(0, |kb| kb / 1024)
}
