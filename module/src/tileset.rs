//! The part of the bundle that the service reads (docs/MODULE_PLAN.md, «Formato en el bundle»),
//! converted from the PMTiles that Planetiler leaves, with only the tiles of the scope from
//! `geo::SCOPE_ZOOM` up:
//!
//! - `hot/index.bin`, pinned in RAM: a header (`GKTI`, the format, the number of tiles), the tile
//!   ids sorted (`u64`) and, in the same order, where each tile is (`offset << 24 | length`, `u64`),
//!   all little-endian; a tile is found with one binary search.
//! - `tiles.bin`, mapped: the tiles, compressed as Planetiler left them, one after another; tiles
//!   with the same content (sea, empty land) are stored once.
//! - `metadata.json`: the metadata of Planetiler (layers, attribution), for `tilejson.json`.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;

use geko_module_sdk::{GekoError, Result};
use serde::{Deserialize, Serialize};

use crate::geo::{self, BBox};
use crate::pmtiles::{self, Archive};

pub const MAGIC: &[u8; 4] = b"GKTI";
pub const FORMAT: u32 = 1;
pub const INDEX: &str = "hot/index.bin";
pub const TILES: &str = "tiles.bin";
pub const METADATA: &str = "metadata.json";

/// Bits of the length of a tile in an entry of the index (16 MiB); the rest, its offset (1 TiB).
pub const LEN_BITS: u32 = 24;

/// What the module keeps of its part in the manifest (`Built::meta`), to load it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub format: u32,
    /// `mvt` or `mlt`.
    pub tile_type: String,
    /// The `Content-Encoding` of every tile: `gzip`, or `identity`.
    pub compression: String,
    pub min_zoom: u8,
    pub max_zoom: u8,
    /// The boxes of the scope.
    pub scope: Vec<BBox>,
    pub tiles: u64,
    /// Different contents among them.
    pub contents: u64,
    pub bytes: u64,
}

/// Converts `pmtiles` into `out` (`INDEX`, `TILES`, `METADATA`).
pub fn convert(pmtiles: &Path, out: &Path, scope: &[BBox]) -> Result<Meta> {
    let mut archive = Archive::open(pmtiles)?;
    let header = archive.header.clone();
    let tile_type = match header.tile_type {
        1 => "mvt",
        6 => "mlt",
        other => return Err(GekoError::Data(format!("{}: tiles of type {other}, neither MVT nor MLT", pmtiles.display()))),
    };
    let compression = match header.tile_compression {
        1 => "identity",
        2 => "gzip",
        other => return Err(GekoError::Data(format!("{}: tiles compressed with {other}, neither gzip nor none", pmtiles.display()))),
    };
    fs::create_dir_all(out.join("hot"))?;
    fs::write(out.join(METADATA), archive.metadata()?)?;

    let mut tiles = BufWriter::with_capacity(1 << 20, File::create(out.join(TILES))?);
    let (mut ids, mut places) = (Vec::new(), Vec::new());
    // Where each content of the archive went, by its offset there.
    let mut stored: HashMap<u64, u64> = HashMap::new();
    let mut written = 0u64;
    for e in archive.entries()? {
        let kept: Vec<u64> = (e.tile_id..e.tile_id + u64::from(e.run))
            .filter(|&id| {
                let (z, x, y) = pmtiles::zxy(id);
                geo::keep(scope, z, x, y)
            })
            .collect();
        if kept.is_empty() {
            continue;
        }
        if e.len >= 1 << LEN_BITS {
            return Err(GekoError::Data(format!("tile {} has {} bytes, more than the index can hold", e.tile_id, e.len)));
        }
        let offset = match stored.get(&e.offset) {
            Some(&offset) => offset,
            None => {
                tiles.write_all(&archive.tile(&e)?)?;
                stored.insert(e.offset, written);
                written += u64::from(e.len);
                written - u64::from(e.len)
            }
        };
        if offset >= 1 << (64 - LEN_BITS) {
            return Err(GekoError::Data("the tiles do not fit in the offsets of the index".into()));
        }
        for id in kept {
            ids.push(id);
            places.push(offset << LEN_BITS | u64::from(e.len));
        }
    }
    tiles.flush()?;
    if ids.windows(2).any(|w| w[0] >= w[1]) {
        return Err(GekoError::Data(format!("{}: tiles out of order", pmtiles.display())));
    }
    let mut index = BufWriter::new(File::create(out.join(INDEX))?);
    index.write_all(MAGIC)?;
    index.write_all(&FORMAT.to_le_bytes())?;
    index.write_all(&(ids.len() as u64).to_le_bytes())?;
    for v in ids.iter().chain(&places) {
        index.write_all(&v.to_le_bytes())?;
    }
    index.flush()?;
    Ok(Meta {
        format: FORMAT,
        tile_type: tile_type.into(),
        compression: compression.into(),
        min_zoom: header.min_zoom,
        max_zoom: header.max_zoom,
        scope: scope.to_vec(),
        tiles: ids.len() as u64,
        contents: stored.len() as u64,
        bytes: written,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pmtiles::tests::archive;
    use crate::pmtiles::tile_id;

    #[test]
    fn keeps_the_scope_and_stores_each_content_once() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.pmtiles");
        let scope = [BBox { west: 1.0, south: 38.5, east: 4.6, north: 40.2 }];
        let (x0, y0, _, _) = scope[0].tiles(14);
        let tiles: Vec<(u64, &[u8])> = vec![
            (tile_id(0, 0, 0), b"world"),
            (tile_id(5, 16, 12), b"sea"),
            (tile_id(6, 0, 0), b"far away"),
            (tile_id(14, x0, y0), b"palma"),
            (tile_id(14, x0 + 1, y0), b"sea"),
        ];
        let mut sorted = tiles.clone();
        sorted.sort();
        std::fs::write(&path, archive(&sorted, 1)).unwrap();
        let out = tmp.path().join("out");
        let meta = convert(&path, &out, &scope).unwrap();
        assert_eq!((meta.tiles, meta.contents, meta.tile_type.as_str(), meta.compression.as_str()), (4, 3, "mvt", "gzip"));

        let index = std::fs::read(out.join(INDEX)).unwrap();
        let data = std::fs::read(out.join(TILES)).unwrap();
        assert_eq!(&index[..4], MAGIC);
        let n = u64::from_le_bytes(index[8..16].try_into().unwrap()) as usize;
        let at = |i: usize| u64::from_le_bytes(index[16 + 8 * i..24 + 8 * i].try_into().unwrap());
        let ids: Vec<u64> = (0..n).map(at).collect();
        let tile = |id: u64| {
            let i = ids.binary_search(&id).ok()?;
            let place = at(n + i);
            let (offset, len) = ((place >> LEN_BITS) as usize, (place & ((1 << LEN_BITS) - 1)) as usize);
            Some(data[offset..offset + len].to_vec())
        };
        for (id, bytes) in &tiles {
            let expected = (*bytes != b"far away").then(|| bytes.to_vec());
            assert_eq!(tile(*id), expected, "{:?}", pmtiles::zxy(*id));
        }
        assert_eq!(data.len() as u64, meta.bytes);
        assert_eq!(std::fs::read(out.join(METADATA)).unwrap(), br#"{"name":"test"}"#);

        let again = tmp.path().join("again");
        convert(&path, &again, &scope).unwrap();
        for f in [INDEX, TILES] {
            assert_eq!(std::fs::read(again.join(f)).unwrap(), std::fs::read(out.join(f)).unwrap());
        }
    }
}
