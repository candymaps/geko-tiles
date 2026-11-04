//! The styles, fonts and sprites (docs/MODULE_PLAN.md, «Estilos»): data, never code of the server.
//! The layers of each Protomaps style (flavor and language) are compiled into the library
//! (`build.rs`); a style is composed on its first request, with the URLs of the tiles, fonts and
//! sprites of the bundle in use, and kept compressed. The fonts and sprites are in the bundle
//! (`assets`), mapped, and served as the tiles are: a lookup and a copy.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::OnceLock;

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use geko_module_sdk::{GekoError, Result};
use memmap2::Mmap;

use crate::assets::{self, Place};

include!(concat!(env!("OUT_DIR"), "/styles.rs"));
static LAYERS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/styles.bin"));

/// The index of a style, by flavor and language.
pub fn find(flavor: &str, lang: &str) -> Option<usize> {
    STYLES.iter().position(|(f, l, _, _)| *f == flavor && *l == lang)
}

/// The flavors and languages of the styles, in order.
pub fn catalog() -> (Vec<&'static str>, Vec<&'static str>) {
    let mut flavors: Vec<&str> = STYLES.iter().map(|s| s.0).collect();
    let mut langs: Vec<&str> = STYLES.iter().map(|s| s.1).collect();
    flavors.dedup();
    langs.sort_unstable();
    langs.dedup();
    (flavors, langs)
}

/// The styles of one bundle, each composed once.
pub struct Styles {
    composed: Vec<OnceLock<Vec<u8>>>,
}

impl Default for Styles {
    fn default() -> Self {
        Self { composed: STYLES.iter().map(|_| OnceLock::new()).collect() }
    }
}

/// What a style points to: where the tiles are, the bundle in use, the encoding of its tiles and
/// the API key of the request, which every URL carries so that the map can load what it names.
pub struct Urls<'a> {
    pub url: &'a str,
    pub version: &'a str,
    pub tile_type: &'a str,
    pub key: Option<&'a str>,
}

impl Urls<'_> {
    /// `&api_key=…`, or nothing without a key.
    fn key_param(&self) -> String {
        self.key.map_or_else(String::new, |k| format!("&api_key={}", encode(k)))
    }

    pub fn tilejson(&self) -> String {
        match self.key {
            Some(k) => format!("{}/tilejson.json?api_key={}", self.url, encode(k)),
            None => format!("{}/tilejson.json", self.url),
        }
    }

    pub fn glyphs(&self) -> String {
        format!("{}/fonts/{{fontstack}}/{{range}}.pbf?v={}{}", self.url, self.version, self.key_param())
    }

    pub fn sprite(&self, name: &str) -> String {
        format!("{}/sprites/{name}?v={}{}", self.url, self.version, self.key_param())
    }

    pub fn tiles(&self) -> String {
        format!("{}/{{z}}/{{x}}/{{y}}?v={}{}", self.url, self.version, self.key_param())
    }
}

/// `text` as a value of a query string.
fn encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

impl Styles {
    /// The style `i` of `STYLES`, compressed with gzip. Kept only if `keep` and without a key: the
    /// key and the origin of a request are whatever it says, so keeping them would let anyone fill
    /// the memory, and a map asks for its style once.
    pub fn get(&self, i: usize, urls: &Urls, keep: bool) -> Cow<'_, [u8]> {
        if !keep || urls.key.is_some() {
            return Cow::Owned(compose(i, urls));
        }
        Cow::Borrowed(self.composed[i].get_or_init(|| compose(i, urls)))
    }
}

fn compose(i: usize, urls: &Urls) -> Vec<u8> {
    let (flavor, lang, offset, len) = STYLES[i];
    let mut layers = String::new();
    GzDecoder::new(&LAYERS[offset..offset + len]).read_to_string(&mut layers).expect("the styles compiled in are valid");
    let text = |s: &str| serde_json::to_string(s).expect("strings serialize");
    // MapLibre GL JS reads MLT only when the source says so.
    let encoding = if urls.tile_type == "mlt" { r#","encoding":"mlt""# } else { "" };
    let style = format!(
        r#"{{"version":8,"name":{},"sources":{{"protomaps":{{"type":"vector","url":{}{encoding}}}}},"glyphs":{},"sprite":{},"layers":{layers}}}"#,
        text(&format!("geko {flavor} ({lang})")),
        text(&urls.tilejson()),
        text(&urls.glyphs()),
        text(&urls.sprite(flavor)),
    );
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(style.as_bytes()).expect("in memory");
    gz.finish().expect("in memory")
}

/// The fonts and sprites of a bundle.
pub struct Assets {
    index: HashMap<String, Place>,
    data: Mmap,
}

impl Assets {
    /// Opens `dir`, checking that every entry of the index points into the data.
    pub fn open(dir: &Path) -> Result<Self> {
        let bad = |what: String| GekoError::Data(format!("{}: {what}", dir.display()));
        let index: HashMap<String, Place> = serde_json::from_slice(&fs::read(dir.join(assets::INDEX))?).map_err(|e| bad(format!("{}: {e}", assets::INDEX)))?;
        // SAFETY: a bundle in use is never modified (AGENTS.md); its files are only replaced by new ones.
        let data = unsafe { Mmap::map(&File::open(dir.join(assets::DATA))?)? };
        if index.values().any(|p| p.offset.checked_add(p.len).is_none_or(|end| end > data.len() as u64)) {
            return Err(bad(format!("{} does not match {}", assets::INDEX, assets::DATA)));
        }
        Ok(Self { index, data })
    }

    /// The bytes of the asset at `path`, and whether they are compressed with gzip.
    pub fn get(&self, path: &str) -> Option<(&[u8], bool)> {
        let p = self.index.get(path)?;
        Some((&self.data[p.offset as usize..(p.offset + p.len) as usize], p.gzip))
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// The paths of every asset, in no order.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.index.keys().map(String::as_str)
    }
}

/// The media type of an asset, by its extension.
pub fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("png") => "image/png",
        Some("json") => "application/json",
        Some("pbf") => "application/x-protobuf",
        _ => "text/plain; charset=utf-8",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_compose_with_the_urls_of_the_bundle() {
        if STYLES.is_empty() {
            assert!(option_env!("GEKO_TILES_PROFILE").is_none(), "the profile has styles");
            return; // Compiled without the profile: no styles.
        }
        let i = find("light", "es").expect("light, in Spanish");
        let styles = Styles::default();
        let urls = Urls { url: "https://maps.example.com/tiles", version: "v1", tile_type: "mlt", key: None };
        let mut text = String::new();
        GzDecoder::new(&*styles.get(i, &urls, true)).read_to_string(&mut text).unwrap();
        let style: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(style["sources"]["protomaps"]["url"], "https://maps.example.com/tiles/tilejson.json");
        assert_eq!(style["sources"]["protomaps"]["encoding"], "mlt");
        assert_eq!(style["glyphs"], "https://maps.example.com/tiles/fonts/{fontstack}/{range}.pbf?v=v1");
        assert_eq!(style["sprite"], "https://maps.example.com/tiles/sprites/light?v=v1");
        assert!(style["layers"].as_array().is_some_and(|l| l.len() > 50));
        let (flavors, langs) = catalog();
        assert_eq!(flavors, ["black", "dark", "grayscale", "light", "white"]);
        assert!(langs.contains(&"es") && langs.len() > 40);

        // A key is in every URL, and not in the style kept for the requests without one.
        let keyed = Urls { key: Some("a b&c"), ..urls };
        let mut text = String::new();
        GzDecoder::new(&*styles.get(i, &keyed, true)).read_to_string(&mut text).unwrap();
        let style: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(style["sources"]["protomaps"]["url"], "https://maps.example.com/tiles/tilejson.json?api_key=a%20b%26c");
        assert_eq!(style["sprite"], "https://maps.example.com/tiles/sprites/light?v=v1&api_key=a%20b%26c");
        let mut text = String::new();
        GzDecoder::new(&*styles.get(i, &Urls { key: None, ..keyed }, true)).read_to_string(&mut text).unwrap();
        assert!(!text.contains("api_key"));
    }

    #[test]
    fn urls_carry_the_key_of_the_request() {
        let urls = Urls { url: "/tiles", version: "v1", tile_type: "mvt", key: Some("k/1") };
        assert_eq!(urls.tilejson(), "/tiles/tilejson.json?api_key=k%2F1");
        assert_eq!(urls.tiles(), "/tiles/{z}/{x}/{y}?v=v1&api_key=k%2F1");
        assert_eq!(urls.glyphs(), "/tiles/fonts/{fontstack}/{range}.pbf?v=v1&api_key=k%2F1");
        assert_eq!(Urls { key: None, ..urls }.tiles(), "/tiles/{z}/{x}/{y}?v=v1");
    }
}
