//! The golden of the tiles (`ci/test.sh --variant golden`, `ci/golden-snapshot.sh`):
//!
//!   cargo run --release -p geko-module-tiles --example golden -- report <data dir>
//!   cargo run --release -p geko-module-tiles --example golden -- osmdata <data dir> <out dir>
//!
//! `report` prints what the build left in `<data dir>/tiles` as JSON, the same bytes for the same
//! tiles: how many and how big by zoom, the sha256 of the whole part, of the tiles of a few places
//! of Baleares and of the fonts and sprites. The golden compares it with `golden/baseline.json`, so
//! a change of the tiles shows where it is. `osmdata` writes the osmdata polygons cut to the scope as
//! the zips they came from, for the frozen data of the golden, each with the record of its download
//! (`<zip>.raw.json`) made to match it: without it, the build would download the whole zip again.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Where the golden looks at single tiles: the towns of each island, the mountains and the sea.
const PLACES: [(&str, f64, f64); 6] = [
    ("Palma", 2.6502, 39.5696),
    ("Eivissa", 1.4326, 38.9089),
    ("Mao", 4.2658, 39.8885),
    ("Formentera", 1.4500, 38.7000),
    ("Tramuntana", 2.7900, 39.8000),
    ("sea", 3.3000, 38.8000),
];
const ZOOMS: [u8; 8] = [0, 2, 4, 6, 8, 10, 12, 14];

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn file_hash(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    Ok(hex(&std::fs::read(path)?))
}

/// The tile of a point at a zoom, in Web Mercator.
fn tile_of(lon: f64, lat: f64, z: u8) -> (u32, u32) {
    let n = f64::from(1u32 << z);
    let lat = lat.to_radians();
    let x = ((lon + 180.0) / 360.0 * n).floor();
    let y = ((1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0 * n).floor();
    (x as u32, y as u32)
}

fn report(data_dir: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    let part = data_dir.join("tiles/bundle");
    let wanted: BTreeMap<(u8, u32, u32), String> = PLACES
        .iter()
        .flat_map(|(name, lon, lat)| ZOOMS.map(|z| { let (x, y) = tile_of(*lon, *lat, z); ((z, x, y), format!("{name} {z}/{x}/{y}")) }))
        .collect();
    let mut places = BTreeMap::new();
    let mut zooms: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut contents = HashSet::new();
    let (mut count, mut stored) = (0u64, 0u64);
    let mut whole = Sha256::new();
    geko_module_tiles::golden::tiles(&part, |z, x, y, bytes| {
        let hash = hex(bytes);
        whole.update(format!("{z}/{x}/{y} {hash}\n"));
        if contents.insert(hash.clone()) {
            stored += bytes.len() as u64;
        }
        let zoom = zooms.entry(format!("z{z:02}")).or_default();
        zoom.0 += 1;
        zoom.1 += bytes.len() as u64;
        count += 1;
        if let Some(name) = wanted.get(&(z, x, y)) {
            places.insert(name.clone(), hash);
        }
    })?;
    // A place whose tile is not in the part (out of the scope, or empty) says so.
    let places: BTreeMap<String, String> = wanted.values().map(|name| (name.clone(), places.get(name).cloned().unwrap_or_else(|| "none".into()))).collect();
    let zooms: BTreeMap<String, Value> = zooms.into_iter().map(|(z, (tiles, bytes))| (z, json!({ "tiles": tiles, "bytes": bytes }))).collect();
    let assets = data_dir.join("tiles/assets");
    let index: BTreeMap<String, Value> = serde_json::from_slice(&std::fs::read(assets.join("index.json"))?)?;
    Ok(json!({
        "tiles": {
            "count": count,
            "contents": contents.len(),
            "stored_bytes": stored,
            "sha256": format!("{:x}", whole.finalize()),
            "zooms": zooms,
            "places": places,
        },
        "metadata_sha256": file_hash(&part.join("metadata.json"))?,
        "assets": {
            "count": index.len(),
            "index_sha256": file_hash(&assets.join("index.json"))?,
            "data_sha256": file_hash(&assets.join("assets.bin"))?,
        },
    }))
}

fn osmdata(data_dir: &Path, out: &Path) -> Result<(), Box<dyn std::error::Error>> {
    geko_module_tiles::golden::zip_osmdata(&data_dir.join("tiles/osmdata"), out)?;
    for entry in std::fs::read_dir(out)? {
        let zip = entry?.path();
        let name = zip.file_name().and_then(|n| n.to_str()).ok_or("a zip without a name")?.to_owned();
        let bytes = std::fs::read(&zip)?;
        let record = format!("{name}.raw.json");
        let mut r: Value = serde_json::from_slice(&std::fs::read(data_dir.join("raw/osmdata").join(&record))?)?;
        r["size"] = json!(bytes.len());
        r["sha256"] = json!(hex(&bytes));
        std::fs::write(out.join(record), serde_json::to_vec_pretty(&r)?)?;
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: golden report <data dir> | golden osmdata <data dir> <out dir>";
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["report", data_dir] => println!("{}", serde_json::to_string_pretty(&report(Path::new(data_dir))?)?),
        ["osmdata", data_dir, out] => osmdata(Path::new(data_dir), Path::new(out))?,
        _ => return Err(usage.into()),
    }
    Ok(())
}
