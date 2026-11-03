//! `geko tiles export`: the tiles of the bundle in use as one PMTiles archive (version 3), for those
//! who serve them with something else (docs/MODULE_PLAN.md, «Formato en el bundle»). The data of the
//! archive is `tiles.bin` as it is; only the directories are made.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

use flate2::Compression;
use flate2::write::GzEncoder;
use geko_module_sdk::bundle::{BundleManifest, current_version};
use geko_module_sdk::module::ModuleName;
use geko_module_sdk::{GekoError, Result};

use crate::pmtiles::{Entry, HEADER_LEN, directory};
use crate::tileset::{self, Meta};

/// What the specification allows the header and the root directory together.
const ROOT_MAX: usize = 16_384 - HEADER_LEN;

fn gzip(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut e = GzEncoder::new(Vec::new(), Compression::default());
    e.write_all(bytes)?;
    Ok(e.finish()?)
}

/// The entries of the index of a bundle: tiles in a row with the same content are one run.
fn entries(index: &[u8], tiles: u64, data_len: u64) -> Result<Vec<Entry>> {
    if index.len() < 16 || &index[..4] != tileset::MAGIC || index.len() as u64 != 16 + 16 * tiles {
        return Err(GekoError::Data(format!("{} is not an index of {tiles} tiles", tileset::INDEX)));
    }
    let word = |i: u64| {
        let at = 16 + 8 * i as usize;
        u64::from_le_bytes(index[at..at + 8].try_into().expect("8 bytes"))
    };
    let mut out: Vec<Entry> = Vec::new();
    for i in 0..tiles {
        let (id, place) = (word(i), word(tiles + i));
        let (offset, len) = (place >> tileset::LEN_BITS, (place & ((1 << tileset::LEN_BITS) - 1)) as u32);
        if offset + u64::from(len) > data_len {
            return Err(GekoError::Data(format!("{} does not match {}", tileset::INDEX, tileset::TILES)));
        }
        match out.last_mut() {
            Some(last) if last.offset == offset && last.len == len && last.tile_id + u64::from(last.run) == id => last.run += 1,
            _ => out.push(Entry { tile_id: id, offset, len, run: 1 }),
        }
    }
    Ok(out)
}

/// The root directory and the leaves: every entry in the root if they fit, else leaves as large as
/// it takes for the root to fit.
fn directories(entries: &[Entry]) -> Result<(Vec<u8>, Vec<u8>)> {
    let root = gzip(&directory(entries))?;
    if root.len() <= ROOT_MAX {
        return Ok((root, Vec::new()));
    }
    let mut size = 4096;
    loop {
        let mut leaves = Vec::new();
        let mut root = Vec::new();
        for chunk in entries.chunks(size) {
            let leaf = gzip(&directory(chunk))?;
            root.push(Entry { tile_id: chunk[0].tile_id, offset: leaves.len() as u64, len: leaf.len() as u32, run: 0 });
            leaves.extend_from_slice(&leaf);
        }
        let root = gzip(&directory(&root))?;
        if root.len() <= ROOT_MAX {
            return Ok((root, leaves));
        }
        size *= 2;
    }
}

/// The header; `parts` are the offset and length of the root, the metadata, the leaves and the data.
fn header(meta: &Meta, parts: [(u64, u64); 4], entries: &[Entry]) -> Vec<u8> {
    let mut h = vec![0u8; HEADER_LEN];
    h[..7].copy_from_slice(b"PMTiles");
    h[7] = 3;
    for (i, (offset, len)) in parts.iter().enumerate() {
        h[8 + 16 * i..16 + 16 * i].copy_from_slice(&offset.to_le_bytes());
        h[16 + 16 * i..24 + 16 * i].copy_from_slice(&len.to_le_bytes());
    }
    let addressed: u64 = entries.iter().map(|e| u64::from(e.run)).sum();
    h[72..80].copy_from_slice(&addressed.to_le_bytes());
    h[80..88].copy_from_slice(&(entries.len() as u64).to_le_bytes());
    h[88..96].copy_from_slice(&meta.contents.to_le_bytes());
    h[96] = u8::from(entries.windows(2).all(|w| w[1].offset > w[0].offset));
    h[97] = 2;
    h[98] = if meta.compression == "gzip" { 2 } else { 1 };
    h[99] = if meta.tile_type == "mlt" { 6 } else { 1 };
    h[100] = meta.min_zoom;
    h[101] = meta.max_zoom;
    let bounds = meta.scope.iter().fold([180.0f64, 90.0, -180.0, -90.0], |a, b| [a[0].min(b.west), a[1].min(b.south), a[2].max(b.east), a[3].max(b.north)]);
    let [w, s, e, n] = if meta.scope.is_empty() { [-180.0, -85.0, 180.0, 85.0] } else { bounds };
    for (at, degrees) in [(102, w), (106, s), (110, e), (114, n), (119, (w + e) / 2.0), (123, (s + n) / 2.0)] {
        h[at..at + 4].copy_from_slice(&((degrees * 1e7) as i32).to_le_bytes());
    }
    h[118] = meta.min_zoom.max(6).min(meta.max_zoom);
    h
}

/// Writes the tiles of the bundle in `root` to `out`.
fn write(root: &Path, meta: &Meta, attribution: &str, out: &Path) -> Result<String> {
    let dir = root.join("tiles");
    let data = dir.join(tileset::TILES);
    let data_len = fs::metadata(&data)?.len();
    let entries = entries(&fs::read(dir.join(tileset::INDEX))?, meta.tiles, data_len)?;
    let (root_directory, leaves) = directories(&entries)?;
    let mut metadata: serde_json::Value = serde_json::from_slice(&fs::read(dir.join(tileset::METADATA))?).map_err(|e| GekoError::Data(format!("{}: {e}", tileset::METADATA)))?;
    // As `tilejson.json`: Protomaps asks modified tiles for another name.
    metadata["name"] = "geko tiles".into();
    metadata["attribution"] = attribution.into();
    let metadata = gzip(&serde_json::to_vec(&metadata).expect("JSON values serialize"))?;

    let mut at = HEADER_LEN as u64;
    let parts = [root_directory.len() as u64, metadata.len() as u64, leaves.len() as u64, data_len].map(|len| {
        let offset = at;
        at += len;
        (offset, len)
    });
    let temporary = out.with_extension("pmtiles.tmp");
    let mut file = io::BufWriter::new(File::create(&temporary)?);
    file.write_all(&header(meta, parts, &entries))?;
    file.write_all(&root_directory)?;
    file.write_all(&metadata)?;
    file.write_all(&leaves)?;
    io::copy(&mut File::open(&data)?, &mut file)?;
    file.into_inner().map_err(io::IntoInnerError::into_error)?.sync_all()?;
    fs::rename(&temporary, out)?;
    Ok(format!("{}: {} tiles, {at} bytes\n", out.display(), meta.tiles))
}

/// `geko tiles export <file>`, with the data directory of the host.
pub fn run(data_dir: &Path, args: &[String], attribution: &str) -> Result<String> {
    let usage = || GekoError::Config("usage: geko tiles export <file.pmtiles>".into());
    let [out] = args else { return Err(usage()) };
    if out.starts_with('-') {
        return Err(usage());
    }
    let bundles = data_dir.join("bundles");
    let version = current_version(&bundles).ok_or_else(|| GekoError::Data("no bundle to export: run geko build".into()))?;
    let root = bundles.join(&version);
    let manifest = BundleManifest::read(&root)?;
    let name = ModuleName::new(crate::NAME)?;
    let module = manifest.module(&name).ok_or_else(|| GekoError::Data(format!("bundle {version} has no tiles")))?;
    let meta: Meta = serde_json::from_value(module.meta.clone()).map_err(|e| GekoError::Data(format!("bundle {version}: the meta of tiles: {e}")))?;
    write(&root, &meta, attribution, Path::new(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::BBox;
    use crate::pmtiles::tests::archive;
    use crate::pmtiles::{Archive, tile_id};

    #[test]
    fn the_archive_has_the_tiles_of_the_bundle() {
        let tmp = tempfile::tempdir().unwrap();
        let scope = [BBox { west: 1.0, south: 38.5, east: 4.6, north: 40.2 }];
        let (x0, y0, _, _) = scope[0].tiles(14);
        let mut tiles: Vec<(u64, &[u8])> = vec![(tile_id(0, 0, 0), b"world"), (tile_id(14, x0, y0), b"palma"), (tile_id(14, x0 + 1, y0), b"sea"), (tile_id(14, x0 + 1, y0 + 1), b"sea")];
        tiles.sort();
        let source = tmp.path().join("t.pmtiles");
        fs::write(&source, archive(&tiles, 1)).unwrap();
        let meta = tileset::convert(&source, &tmp.path().join("b/tiles"), &scope).unwrap();

        let out = tmp.path().join("out.pmtiles");
        write(&tmp.path().join("b"), &meta, "© OSM", &out).unwrap();
        let mut a = Archive::open(&out).unwrap();
        assert_eq!((a.header.tile_type, a.header.tile_compression, a.header.min_zoom, a.header.max_zoom), (1, 2, meta.min_zoom, 14));
        let metadata: serde_json::Value = serde_json::from_slice(&a.metadata().unwrap()).unwrap();
        assert_eq!((metadata["name"].as_str(), metadata["attribution"].as_str()), (Some("geko tiles"), Some("© OSM")));
        let entries = a.entries().unwrap();
        let mut read = Vec::new();
        for e in &entries {
            let bytes = a.tile(e).unwrap();
            read.extend((0..u64::from(e.run)).map(|i| (e.tile_id + i, bytes.clone())));
        }
        assert_eq!(read, tiles.iter().map(|(id, b)| (*id, b.to_vec())).collect::<Vec<_>>());
    }

    #[test]
    fn tiles_in_a_row_with_the_same_content_are_one_run() {
        let mut index = Vec::from(*tileset::MAGIC);
        index.extend(tileset::FORMAT.to_le_bytes());
        index.extend(3u64.to_le_bytes());
        for id in [5u64, 6, 8] {
            index.extend(id.to_le_bytes());
        }
        for _ in 0..3 {
            index.extend(((10u64 << tileset::LEN_BITS) | 4).to_le_bytes());
        }
        let e = entries(&index, 3, 14).unwrap();
        assert_eq!(e, [Entry { tile_id: 5, offset: 10, len: 4, run: 2 }, Entry { tile_id: 8, offset: 10, len: 4, run: 1 }]);
        assert!(entries(&index, 3, 13).is_err());
        assert!(entries(&index, 4, 14).is_err());
    }

    #[test]
    fn many_entries_go_to_leaves_and_the_root_fits() {
        // Offsets far apart and ids out of step, so that gzip cannot make them small.
        let mut x = 1u64;
        let mut id = 0;
        let many: Vec<Entry> = (0..400_000u64)
            .map(|_| {
                x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                id += 1 + (x >> 50);
                Entry { tile_id: id, offset: x >> 20, len: (x >> 40) as u32 & 0xffff, run: 1 }
            })
            .collect();
        let (root, leaves) = directories(&many).unwrap();
        assert!(root.len() <= ROOT_MAX && !leaves.is_empty());
    }
}
