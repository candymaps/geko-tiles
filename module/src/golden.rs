//! What the golden of the tiles reads and freezes (`examples/golden.rs`: `ci/test.sh --variant
//! golden` and `ci/golden-snapshot.sh`). The service does not use it.

use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::path::Path;

use geko_module_sdk::{GekoError, Result};

use crate::inputs::{LAND, WATER};
use crate::{pmtiles, tileset};

/// Calls `f` with every tile of the part in `dir` (`tiles/` of a bundle, or the `bundle` stage), in
/// the order of their ids: its zoom, column, row and bytes as stored.
pub fn tiles(dir: &Path, mut f: impl FnMut(u8, u32, u32, &[u8])) -> Result<()> {
    let bad = |what: &str| GekoError::Data(format!("{}: {what}", dir.display()));
    let index = fs::read(dir.join(tileset::INDEX))?;
    let data = fs::read(dir.join(tileset::TILES))?;
    if index.len() < 16 || &index[..4] != tileset::MAGIC || index[4..8] != tileset::FORMAT.to_le_bytes() {
        return Err(bad("not an index of this format"));
    }
    let n = u64::from_le_bytes(index[8..16].try_into().expect("8 bytes")) as usize;
    let words: Vec<u64> = index[16..].as_chunks::<8>().0.iter().map(|w| u64::from_le_bytes(*w)).collect();
    if words.len() != 2 * n {
        return Err(bad("the index does not hold its tiles"));
    }
    let (ids, places) = words.split_at(n);
    for (id, place) in ids.iter().zip(places) {
        let (offset, len) = ((place >> tileset::LEN_BITS) as usize, (place & ((1 << tileset::LEN_BITS) - 1)) as usize);
        let bytes = data.get(offset..offset + len).ok_or_else(|| bad("an entry of the index is out of the tiles"))?;
        let (z, x, y) = pmtiles::zxy(*id);
        f(z, x, y, bytes);
    }
    Ok(())
}

/// Writes the osmdata polygons cut to the scope (the `osmdata` stage, `cut_dir`) as the zips they
/// came from, into `out_dir`: cutting them again to the same scope gives the same files, so the
/// golden freezes a few MB instead of the 1.8 GB of the whole planet.
pub fn zip_osmdata(cut_dir: &Path, out_dir: &Path) -> Result<()> {
    fs::create_dir_all(out_dir)?;
    for name in [WATER, LAND] {
        let out = File::create(out_dir.join(format!("{name}.zip")))?;
        let mut zip = zip::ZipWriter::new(BufWriter::new(out));
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for ext in ["shp", "shx", "dbf", "prj", "cpg"] {
            let path = cut_dir.join(format!("{name}.{ext}"));
            if !path.exists() {
                continue;
            }
            zip.start_file(format!("{name}/{name}.{ext}"), options).map_err(|e| GekoError::Data(e.to_string()))?;
            io::copy(&mut File::open(&path)?, &mut zip)?;
        }
        zip.finish().map_err(|e| GekoError::Data(e.to_string()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::BBox;
    use crate::pmtiles::tests::archive;
    use crate::pmtiles::tile_id;
    use crate::shapefile;

    #[test]
    fn reads_every_tile_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let mut tiles: Vec<(u64, &[u8])> = vec![(tile_id(0, 0, 0), b"world"), (tile_id(1, 1, 0), b"sea"), (tile_id(1, 0, 1), b"sea")];
        tiles.sort();
        let pmtiles = tmp.path().join("t.pmtiles");
        fs::write(&pmtiles, archive(&tiles, 1)).unwrap();
        tileset::convert(&pmtiles, &tmp.path().join("out"), &[BBox { west: 1.0, south: 38.5, east: 4.6, north: 40.2 }]).unwrap();
        let mut read = Vec::new();
        super::tiles(&tmp.path().join("out"), |z, x, y, b| read.push(((z, x, y), b.to_vec()))).unwrap();
        let expected: Vec<_> = tiles.iter().map(|(id, b)| (pmtiles::zxy(*id), b.to_vec())).collect();
        assert_eq!(read, expected);
    }

    #[test]
    fn the_frozen_cut_cuts_to_itself() {
        let tmp = tempfile::tempdir().unwrap();
        let [raw, cut, frozen, again] = ["raw", "cut", "frozen", "again"].map(|d| tmp.path().join(d));
        for dir in [&raw, &cut, &again] {
            fs::create_dir_all(dir).unwrap();
        }
        let scope = [[0.0, 0.0, 1.5, 1.5]];
        for name in [WATER, LAND] {
            let zip = raw.join(format!("{name}.zip"));
            fs::write(&zip, shapefile::tests::zip_of(&[[0.0, 0.0, 1.0, 1.0], [5.0, 5.0, 6.0, 6.0], [0.5, 0.5, 2.0, 2.0]])).unwrap();
            assert_eq!(shapefile::cut(&zip, &cut, name, &scope).unwrap().kept, 2);
        }
        zip_osmdata(&cut, &frozen).unwrap();
        for name in [WATER, LAND] {
            let c = shapefile::cut(&frozen.join(format!("{name}.zip")), &again, name, &scope).unwrap();
            assert_eq!((c.kept, c.total), (2, 2));
            for ext in ["shp", "shx", "dbf"] {
                let file = format!("{name}.{ext}");
                assert_eq!(fs::read(again.join(&file)).unwrap(), fs::read(cut.join(&file)).unwrap(), "{file}");
            }
        }
    }
}
