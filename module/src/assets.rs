//! The fonts and sprites of the styles in the bundle (docs/MODULE_PLAN.md, «Estilos»), packed as
//! the tiles are, for the service to answer each one with a lookup and a copy:
//!
//! - `assets/assets.bin`, mapped: the contents, one after another, each one once. Those that gzip
//!   makes smaller (the glyphs, the JSON) are stored compressed; the PNG, as they are.
//! - `assets/index.json`: by path (`fonts/<font stack>/<range>.pbf`, `sprites/<name>`), where its
//!   content is and whether it is compressed.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::GzEncoder;
use geko_module_sdk::{GekoError, Result};
use serde::{Deserialize, Serialize};

pub const DATA: &str = "assets.bin";
pub const INDEX: &str = "index.json";

/// Where an asset is in `DATA`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub offset: u64,
    pub len: u64,
    pub gzip: bool,
}

/// What a pack reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Packed {
    pub assets: usize,
    pub contents: usize,
    pub bytes: u64,
}

/// One asset to pack: its path in the bundle and the file it comes from.
pub struct Source {
    pub path: String,
    pub file: PathBuf,
}

/// Packs `sources` into `out`; `links`, by path, take the content of the asset at their target.
pub fn pack(sources: &[Source], links: &[(String, String)], out: &Path) -> Result<Packed> {
    fs::create_dir_all(out)?;
    let mut data = BufWriter::new(File::create(out.join(DATA))?);
    let mut index: BTreeMap<String, Place> = BTreeMap::new();
    // By content and whether it may be compressed: a PNG is never stored compressed.
    let mut stored: HashMap<(Vec<u8>, bool), Place> = HashMap::new();
    let mut written = 0u64;
    let mut sorted: Vec<&Source> = sources.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    for source in sorted {
        let bytes = fs::read(&source.file).map_err(|e| GekoError::Data(format!("{}: {e}", source.file.display())))?;
        let compressible = !source.path.ends_with(".png");
        let key = (bytes, compressible);
        let place = match stored.get(&key) {
            Some(&place) => place,
            None => {
                let bytes = &key.0;
                let compressed = if compressible { Some(gzip(bytes)?) } else { None };
                let (body, gzip) = match &compressed {
                    Some(c) if c.len() < bytes.len() => (c.as_slice(), true),
                    _ => (bytes.as_slice(), false),
                };
                data.write_all(body)?;
                let place = Place { offset: written, len: body.len() as u64, gzip };
                written += place.len;
                stored.insert(key, place);
                place
            }
        };
        index.insert(source.path.clone(), place);
    }
    for (path, target) in links {
        let place = *index.get(target).ok_or_else(|| GekoError::Data(format!("{path} links to {target}, which is not packed")))?;
        index.insert(path.clone(), place);
    }
    data.flush()?;
    fs::write(out.join(INDEX), serde_json::to_vec(&index).map_err(|e| GekoError::Other(e.into()))?)?;
    Ok(Packed { assets: index.len(), contents: stored.len(), bytes: written })
}

fn gzip(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut gz = GzEncoder::new(Vec::with_capacity(bytes.len() / 2), Compression::best());
    gz.write_all(bytes)?;
    Ok(gz.finish()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_each_content_once_and_compresses_what_shrinks() {
        let tmp = tempfile::tempdir().unwrap();
        let file = |name: &str, bytes: &[u8]| {
            let path = tmp.path().join(name);
            fs::write(&path, bytes).unwrap();
            path
        };
        let glyphs = vec![7u8; 4096];
        let sources = [
            Source { path: "fonts/A/0-255.pbf".into(), file: file("a", &glyphs) },
            Source { path: "fonts/B/0-255.pbf".into(), file: file("b", &glyphs) },
            Source { path: "sprites/light.png".into(), file: file("c", &glyphs) },
            Source { path: "sprites/light.json".into(), file: file("d", b"{}") },
        ];
        let links = [("fonts/C/0-255.pbf".to_owned(), "fonts/A/0-255.pbf".to_owned())];
        let out = tmp.path().join("out");
        let packed = pack(&sources, &links, &out).unwrap();
        // The glyphs once compressed and once as a PNG, as they are; the tiny JSON, as it is.
        assert_eq!((packed.assets, packed.contents), (5, 3));
        let index: BTreeMap<String, Place> = serde_json::from_slice(&fs::read(out.join(INDEX)).unwrap()).unwrap();
        let (a, b, c) = (index["fonts/A/0-255.pbf"], index["fonts/B/0-255.pbf"], index["fonts/C/0-255.pbf"]);
        assert!(a == b && a == c && a.gzip && a.len < 4096);
        assert!(!index["sprites/light.png"].gzip && index["sprites/light.png"].len == 4096);
        assert!(!index["sprites/light.json"].gzip);
        assert_eq!(fs::metadata(out.join(DATA)).unwrap().len(), packed.bytes);

        let again = tmp.path().join("again");
        pack(&sources, &links, &again).unwrap();
        for f in [DATA, INDEX] {
            assert_eq!(fs::read(again.join(f)).unwrap(), fs::read(out.join(f)).unwrap());
        }
        assert!(pack(&sources, &[("x".into(), "missing".into())], &tmp.path().join("bad")).is_err());
    }
}
