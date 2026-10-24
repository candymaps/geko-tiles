//! The service (docs/MODULE_PLAN.md, «Servicio»): the part of the bundle that `tileset` wrote,
//! loaded once and swapped whole when a new bundle arrives. A tile is one binary search in the
//! index, pinned in RAM, and a copy of its bytes from `tiles.bin`, mapped: nothing is decoded or
//! compressed, and the body is the only allocation.

use std::fmt::Write;
use std::fs::{self, File};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arc_swap::ArcSwapOption;
use geko_module_sdk::module::{BundleView, Request, Response};
use geko_module_sdk::{GekoError, Result};
use memmap2::Mmap;
use serde_json::json;

use crate::pmtiles::tile_id;
use crate::tileset::{self, Meta};

/// A tile of a bundle: the bundle id is in its URL, so it never changes.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";
/// What may change with the next bundle: the TileJSON, a tile asked without the id of the bundle in
/// use, an error.
const REVALIDATE: &str = "no-cache";

/// The part of one bundle, ready to serve.
struct Loaded {
    /// The id of the bundle, in the URLs of `tilejson.json`.
    version: String,
    /// The ids of the tiles, sorted, and where each one is (`offset << LEN_BITS | len`).
    ids: Vec<u64>,
    places: Vec<u64>,
    tiles: Mmap,
    content_type: &'static str,
    gzip: bool,
    max_zoom: u8,
    tilejson: Vec<u8>,
}

#[derive(Default)]
struct Counters {
    tiles: AtomicU64,
    empty: AtomicU64,
    errors: AtomicU64,
    bytes: AtomicU64,
}

pub struct Server {
    /// Where clients fetch the tiles, as `tilejson.json` writes it (`geko-tiles.toml`, `url`).
    url: String,
    loaded: ArcSwapOption<Loaded>,
    counters: Counters,
}

impl Server {
    pub fn new(url: &str) -> Self {
        Self { url: url.trim_end_matches('/').to_owned(), loaded: ArcSwapOption::empty(), counters: Counters::default() }
    }

    /// Loads the part of `view`; the bundle in use, if any, stays until it has loaded.
    pub fn load(&self, view: &BundleView) -> Result<()> {
        let meta: Meta = serde_json::from_value(view.module.meta.clone()).map_err(|e| GekoError::Data(format!("bundle {}: the meta of tiles: {e}", view.manifest.version)))?;
        let loaded = open(&view.root.join("tiles"), &meta, &view.manifest.version, &self.url)?;
        tracing::info!(bundle = %loaded.version, tiles = loaded.ids.len(), "tiles loaded");
        self.loaded.store(Some(Arc::new(loaded)));
        Ok(())
    }

    pub fn shutdown(&self) {
        self.loaded.store(None);
    }

    /// Answers `{z}/{x}/{y}` and `tilejson.json`.
    pub fn handle(&self, request: &Request) -> Response {
        let loaded = self.loaded.load();
        let response = match (&*loaded, request.route.as_str()) {
            (None, _) => Response::error(503, "no tiles bundle loaded"),
            (Some(l), "tilejson.json") => Response::new(200, "application/json", l.tilejson.clone()).with_header("Cache-Control", REVALIDATE),
            (Some(l), "{z}/{x}/{y}") => self.tile(l, request),
            (Some(_), other) => Response::error(404, &format!("no route {other}")),
        };
        if response.status >= 400 {
            self.counters.errors.fetch_add(1, Ordering::Relaxed);
            return response.with_header("Cache-Control", REVALIDATE);
        }
        response
    }

    fn tile(&self, l: &Loaded, request: &Request) -> Response {
        let param = |name: &str| request.params.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());
        let Some((z, x, y)) = coordinates(param("z"), param("x"), param("y")) else {
            return Response::error(400, "a tile is /tiles/{z}/{x}/{y}, with 0 <= x, y < 2^z");
        };
        if z > l.max_zoom {
            return Response::error(404, &format!("the tiles go up to zoom {}", l.max_zoom));
        }
        // Cached for good only under the URL of the bundle that answers it.
        let current = request.query.iter().any(|(k, v)| k == "v" && *v == l.version);
        let cache = if current { IMMUTABLE } else { REVALIDATE };
        let Ok(i) = l.ids.binary_search(&tile_id(z, x, y)) else {
            self.counters.empty.fetch_add(1, Ordering::Relaxed);
            return Response::new(204, l.content_type, Vec::new()).with_header("Cache-Control", cache);
        };
        let place = l.places[i];
        let (offset, len) = ((place >> tileset::LEN_BITS) as usize, (place & ((1 << tileset::LEN_BITS) - 1)) as usize);
        let body = l.tiles[offset..offset + len].to_vec();
        self.counters.tiles.fetch_add(1, Ordering::Relaxed);
        self.counters.bytes.fetch_add(len as u64, Ordering::Relaxed);
        let response = Response::new(200, l.content_type, body).with_header("Cache-Control", cache);
        if l.gzip { response.with_header("Content-Encoding", "gzip") } else { response }
    }

    pub fn metrics(&self, out: &mut String) {
        let c = &self.counters;
        out.push_str("# HELP geko_tiles_requests_total Tile requests answered, by result: a tile, no tile (204) or an error.\n# TYPE geko_tiles_requests_total counter\n");
        for (result, n) in [("tile", &c.tiles), ("empty", &c.empty), ("error", &c.errors)] {
            let _ = writeln!(out, "geko_tiles_requests_total{{result=\"{result}\"}} {}", n.load(Ordering::Relaxed));
        }
        out.push_str("# HELP geko_tiles_sent_bytes_total Bytes of the tiles sent, as stored (compressed).\n# TYPE geko_tiles_sent_bytes_total counter\n");
        let _ = writeln!(out, "geko_tiles_sent_bytes_total {}", c.bytes.load(Ordering::Relaxed));
        if let Some(l) = &*self.loaded.load() {
            out.push_str("# HELP geko_tiles_loaded The tiles of the bundle in use.\n# TYPE geko_tiles_loaded gauge\n");
            let _ = writeln!(out, "geko_tiles_loaded {}", l.ids.len());
            out.push_str("# HELP geko_tiles_index_bytes Memory of the index of the bundle in use.\n# TYPE geko_tiles_index_bytes gauge\n");
            let _ = writeln!(out, "geko_tiles_index_bytes {}", 16 * l.ids.len());
        }
    }
}

/// `z`, `x` and `y` of a tile; `y` may end in the extension of the tiles (`.mvt`, `.pbf`, `.mlt`).
fn coordinates(z: Option<&str>, x: Option<&str>, y: Option<&str>) -> Option<(u8, u32, u32)> {
    let y = y?;
    let y = [".mvt", ".pbf", ".mlt"].iter().find_map(|e| y.strip_suffix(e)).unwrap_or(y);
    let number = |s: &str| if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) { s.parse::<u32>().ok() } else { None };
    let z = u8::try_from(number(z?)?).ok().filter(|&z| z < 32)?;
    let (x, y) = (number(x?)?, number(y)?);
    (u64::from(x) < 1 << z && u64::from(y) < 1 << z).then_some((z, x, y))
}

/// Opens the part of a bundle in `dir`, checking that every entry of the index points into
/// `tiles.bin`: serving never slices out of it.
fn open(dir: &Path, meta: &Meta, version: &str, url: &str) -> Result<Loaded> {
    let bad = |what: String| GekoError::Data(format!("{}: {what}", dir.display()));
    if meta.format != tileset::FORMAT {
        return Err(bad(format!("tiles of format {}, this module reads {}", meta.format, tileset::FORMAT)));
    }
    let index = fs::read(dir.join(tileset::INDEX))?;
    if index.len() < 16 || &index[..4] != tileset::MAGIC || index[4..8] != tileset::FORMAT.to_le_bytes() {
        return Err(bad(format!("{} is not an index of format {}", tileset::INDEX, tileset::FORMAT)));
    }
    let n = u64::from_le_bytes(index[8..16].try_into().expect("8 bytes"));
    if n != meta.tiles || (index.len() - 16) as u64 != 16 * n {
        return Err(bad(format!("{} does not hold the {} tiles of the meta", tileset::INDEX, meta.tiles)));
    }
    let words: Vec<u64> = index[16..].as_chunks::<8>().0.iter().map(|w| u64::from_le_bytes(*w)).collect();
    let (ids, places) = words.split_at(n as usize);
    let file = File::open(dir.join(tileset::TILES))?;
    // SAFETY: a bundle in use is never modified (AGENTS.md); its files are only replaced by new ones.
    let tiles = unsafe { Mmap::map(&file)? };
    let size = tiles.len() as u64;
    if ids.windows(2).any(|w| w[0] >= w[1]) || places.iter().any(|p| (p >> tileset::LEN_BITS) + (p & ((1 << tileset::LEN_BITS) - 1)) > size) {
        return Err(bad(format!("{} does not match {}", tileset::INDEX, tileset::TILES)));
    }
    let metadata: serde_json::Value = serde_json::from_slice(&fs::read(dir.join(tileset::METADATA))?).map_err(|e| bad(format!("{}: {e}", tileset::METADATA)))?;
    let (content_type, gzip) = (
        match meta.tile_type.as_str() {
            "mvt" => "application/vnd.mapbox-vector-tile",
            // MLT has no registered media type yet.
            _ => "application/octet-stream",
        },
        meta.compression == "gzip",
    );
    Ok(Loaded {
        version: version.to_owned(),
        ids: ids.to_vec(),
        places: places.to_vec(),
        tiles,
        content_type,
        gzip,
        max_zoom: meta.max_zoom,
        tilejson: tilejson(meta, &metadata, version, url),
    })
}

/// The TileJSON 3.0 of a bundle: what a client needs to add the tiles as a source.
fn tilejson(meta: &Meta, metadata: &serde_json::Value, version: &str, url: &str) -> Vec<u8> {
    let bounds = meta.scope.iter().fold(None::<[f64; 4]>, |acc, b| {
        Some(acc.map_or([b.west, b.south, b.east, b.north], |a| [a[0].min(b.west), a[1].min(b.south), a[2].max(b.east), a[3].max(b.north)]))
    });
    let mut tilejson = json!({
        "tilejson": "3.0.0",
        "name": metadata.get("name"),
        "description": metadata.get("description"),
        "version": metadata.get("version"),
        "attribution": metadata.get("attribution"),
        "scheme": "xyz",
        "tiles": [format!("{url}/{{z}}/{{x}}/{{y}}?v={version}")],
        "minzoom": meta.min_zoom,
        "maxzoom": meta.max_zoom,
        "vector_layers": metadata.get("vector_layers"),
        // Not part of TileJSON: MapLibre GL JS reads MLT only with `encoding: "mlt"` on the source.
        "encoding": meta.tile_type,
    });
    if let Some([w, s, e, n]) = bounds {
        tilejson["bounds"] = json!([w, s, e, n]);
        tilejson["center"] = json!([(w + e) / 2.0, (s + n) / 2.0, meta.min_zoom.max(6)]);
    }
    serde_json::to_vec(&tilejson).expect("JSON values serialize")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::BBox;
    use crate::pmtiles::tests::archive;
    use geko_module_sdk::bundle::{BundleManifest, BundleModule};
    use geko_module_sdk::module::{CONTRACT, ModuleName};

    fn view(root: &Path, version: &str, meta: &Meta) -> BundleView {
        let module = BundleModule { name: ModuleName::new("tiles").unwrap(), version: "0".into(), contract: CONTRACT, meta: serde_json::to_value(meta).unwrap() };
        let manifest = BundleManifest { format: 1, version: version.into(), created_unix: 0, modules: vec![module.clone()], licenses: vec![], artifacts: vec![] };
        BundleView { root: root.to_path_buf(), manifest, module }
    }

    fn get(server: &Server, route: &str, params: &[(&str, &str)], query: &[(&str, &str)]) -> Response {
        let pairs = |p: &[(&str, &str)]| p.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        server.handle(&Request { route: route.into(), params: pairs(params), query: pairs(query), ..Default::default() })
    }

    fn header<'a>(r: &'a Response, name: &str) -> Option<&'a str> {
        r.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    #[test]
    fn serves_the_tiles_of_the_bundle_in_use() {
        let tmp = tempfile::tempdir().unwrap();
        let scope = [BBox { west: 1.0, south: 38.5, east: 4.6, north: 40.2 }];
        let (x0, y0, _, _) = scope[0].tiles(14);
        let mut tiles: Vec<(u64, &[u8])> = vec![(tile_id(0, 0, 0), b"world"), (tile_id(14, x0, y0), b"palma"), (tile_id(14, x0 + 1, y0), b"sea")];
        tiles.sort();
        let pmtiles = tmp.path().join("t.pmtiles");
        fs::write(&pmtiles, archive(&tiles, 1)).unwrap();
        let meta = tileset::convert(&pmtiles, &tmp.path().join("a/tiles"), &scope).unwrap();

        let server = Server::new("https://cdn.example.com/tiles/");
        assert_eq!(get(&server, "{z}/{x}/{y}", &[("z", "0"), ("x", "0"), ("y", "0")], &[]).status, 503);
        server.load(&view(&tmp.path().join("a"), "b1", &meta)).unwrap();

        let (x, y) = (x0.to_string(), y0.to_string());
        let r = get(&server, "{z}/{x}/{y}", &[("z", "14"), ("x", &x), ("y", &format!("{y}.mvt"))], &[("v", "b1")]);
        assert_eq!((r.status, r.body.as_slice(), r.content_type.as_str()), (200, &b"palma"[..], "application/vnd.mapbox-vector-tile"));
        assert_eq!((header(&r, "Content-Encoding"), header(&r, "Cache-Control")), (Some("gzip"), Some(IMMUTABLE)));
        let r = get(&server, "{z}/{x}/{y}", &[("z", "0"), ("x", "0"), ("y", "0")], &[("v", "old")]);
        assert_eq!((r.status, r.body.as_slice(), header(&r, "Cache-Control")), (200, &b"world"[..], Some(REVALIDATE)));
        let r = get(&server, "{z}/{x}/{y}", &[("z", "14"), ("x", "0"), ("y", "0")], &[("v", "b1")]);
        assert_eq!((r.status, r.body.len(), header(&r, "Cache-Control")), (204, 0, Some(IMMUTABLE)));
        assert_eq!(get(&server, "{z}/{x}/{y}", &[("z", "15"), ("x", "0"), ("y", "0")], &[]).status, 404);
        for (z, x, y) in [("1", "2", "0"), ("-1", "0", "0"), ("a", "0", "0"), ("40", "0", "0"), ("2", "+1", "0"), ("2", "1", "")] {
            let r = get(&server, "{z}/{x}/{y}", &[("z", z), ("x", x), ("y", y)], &[]);
            assert_eq!(r.status, 400, "{z}/{x}/{y}");
        }

        let r = get(&server, "tilejson.json", &[], &[]);
        let tilejson: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(tilejson["tiles"][0], "https://cdn.example.com/tiles/{z}/{x}/{y}?v=b1");
        assert_eq!((tilejson["minzoom"].as_u64(), tilejson["maxzoom"].as_u64()), (Some(0), Some(14)));
        assert_eq!(tilejson["bounds"], json!([1.0, 38.5, 4.6, 40.2]));

        let mut metrics = String::new();
        server.metrics(&mut metrics);
        assert!(metrics.contains("geko_tiles_requests_total{result=\"tile\"} 2\n"), "{metrics}");
        assert!(metrics.contains("geko_tiles_requests_total{result=\"empty\"} 1\n"), "{metrics}");
        assert!(metrics.contains("geko_tiles_loaded 3\n"), "{metrics}");

        // A broken bundle leaves the one in use.
        let broken = tmp.path().join("b");
        fs::create_dir_all(broken.join("tiles/hot")).unwrap();
        for f in [tileset::INDEX, tileset::TILES, tileset::METADATA] {
            fs::copy(tmp.path().join("a/tiles").join(f), broken.join("tiles").join(f)).unwrap();
        }
        fs::write(broken.join("tiles").join(tileset::TILES), b"short").unwrap();
        assert!(server.load(&view(&broken, "b2", &meta)).is_err());
        assert_eq!(get(&server, "tilejson.json", &[], &[]).body, r.body);
    }
}
