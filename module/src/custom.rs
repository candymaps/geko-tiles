//! The styles of the user (docs/MODULE_PLAN.md, «Estilos propios»): data, as those of Protomaps, but
//! outside the bundle, so that one changes without building the tiles again. Each is a file of
//! `<data dir>/tiles-styles/`, on the volume every replica shares: it is read on each request, so
//! all of them answer the last one saved. A map asks for its style once, so the read costs nothing
//! next to its tiles.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use geko_module_sdk::Result;
use serde_json::{Value, json};

use crate::styles::Urls;

/// The largest style: those of Protomaps are about 100 kB.
pub const MAX_BYTES: usize = 1 << 20;
pub const MAX_STYLES: usize = 200;
/// The source of a style that the module points to the tiles of the bundle in use.
const SOURCE: &str = "protomaps";
/// Without a `sprite` of its own, a style has the sprites of this flavor of Protomaps.
const SPRITE: &str = "light";
const LAYER_TYPES: [&str; 11] = ["background", "fill", "line", "symbol", "raster", "circle", "fill-extrusion", "heatmap", "hillshade", "sky", "color-relief"];

static TEMPORARY: AtomicU64 = AtomicU64::new(0);

/// Why a style cannot be saved, for the one who sent it.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    Invalid(String),
    Full,
}

pub struct Store {
    dir: PathBuf,
    editable: bool,
}

impl Store {
    pub fn new(data_dir: &Path, editable: bool) -> Self {
        Self { dir: data_dir.join("tiles-styles"), editable }
    }

    pub fn editable(&self) -> bool {
        self.editable
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.json"))
    }

    /// The names of the styles, in order.
    pub fn list(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(&self.dir) else { return Vec::new() };
        let mut names: Vec<String> = entries.filter_map(|e| e.ok()?.file_name().to_str()?.strip_suffix(".json").map(str::to_owned)).filter(|n| valid_name(n)).collect();
        names.sort_unstable();
        names
    }

    /// The style `name`, as it was saved; `None` if there is none.
    pub fn read(&self, name: &str) -> Option<Value> {
        if !valid_name(name) {
            return None;
        }
        serde_json::from_slice(&fs::read(self.path(name)).ok()?).ok()
    }

    /// Saves `body` as the style `name`, whole or not at all: a replica never reads half of it.
    pub fn save(&self, name: &str, body: &[u8]) -> Result<std::result::Result<(), Refused>> {
        if let Err(why) = check(name, body) {
            return Ok(Err(Refused::Invalid(why)));
        }
        if !self.path(name).exists() && self.list().len() >= MAX_STYLES {
            return Ok(Err(Refused::Full));
        }
        fs::create_dir_all(&self.dir)?;
        let temporary = self.dir.join(format!(".{}-{}.tmp", std::process::id(), TEMPORARY.fetch_add(1, Ordering::Relaxed)));
        let mut file = fs::File::create(&temporary)?;
        file.write_all(body)?;
        file.sync_all()?;
        fs::rename(&temporary, self.path(name))?;
        Ok(Ok(()))
    }

    /// Deletes the style `name`; false if there was none.
    pub fn delete(&self, name: &str) -> Result<bool> {
        if !valid_name(name) {
            return Ok(false);
        }
        match fs::remove_file(self.path(name)) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
}

/// A name that is safe as a file and as a part of a URL.
pub fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// What the module can check of a style without a renderer: that a client can load it and that its
/// layers name sources it has. The values of paint and layout are for the client to judge.
fn check(name: &str, body: &[u8]) -> std::result::Result<(), String> {
    if !valid_name(name) {
        return Err("a style is named with up to 64 lowercase letters, digits, `-` and `_`, from a letter or a digit".into());
    }
    if body.len() > MAX_BYTES {
        return Err(format!("a style has at most {MAX_BYTES} bytes"));
    }
    let style: Value = serde_json::from_slice(body).map_err(|e| format!("not JSON: {e}"))?;
    if !style.is_object() {
        return Err("a style is a JSON object".into());
    }
    if style["version"] != 8 {
        return Err("`version` must be 8".into());
    }
    if !(style["sources"].is_null() || style["sources"].is_object()) {
        return Err("`sources` must be an object".into());
    }
    if !(style["glyphs"].is_null() || style["glyphs"].is_string()) {
        return Err("`glyphs` must be a URL".into());
    }
    if !(style["sprite"].is_null() || style["sprite"].is_string() || style["sprite"].is_array()) {
        return Err("`sprite` must be a URL or a list of sprites".into());
    }
    let Some(layers) = style["layers"].as_array() else { return Err("`layers` must be a list".into()) };
    let mut ids = std::collections::HashSet::new();
    for (i, layer) in layers.iter().enumerate() {
        let Some(id) = layer["id"].as_str() else { return Err(format!("layer {i} has no `id`")) };
        if !ids.insert(id) {
            return Err(format!("two layers are `{id}`"));
        }
        let kind = layer["type"].as_str().unwrap_or_default();
        if !LAYER_TYPES.contains(&kind) {
            return Err(format!("layer `{id}`: no type {kind:?}"));
        }
        if kind == "background" || kind == "sky" {
            continue;
        }
        let Some(source) = layer["source"].as_str() else { return Err(format!("layer `{id}` has no `source`")) };
        if source != SOURCE && style["sources"].get(source).is_none() {
            return Err(format!("layer `{id}`: no source `{source}`, and the tiles of the bundle are `{SOURCE}`"));
        }
    }
    Ok(())
}

/// A saved style, with the URLs of the bundle in use: the source `protomaps` is always its tiles,
/// and the fonts and sprites are those of the bundle unless the style names its own.
pub fn compose(mut style: Value, name: &str, urls: &Urls) -> Vec<u8> {
    let mut source = json!({ "type": "vector", "url": urls.tilejson() });
    if urls.tile_type == "mlt" {
        source["encoding"] = json!("mlt");
    }
    if !style["sources"].is_object() {
        style["sources"] = json!({});
    }
    style["sources"][SOURCE] = source;
    if style["glyphs"].is_null() {
        style["glyphs"] = json!(urls.glyphs());
    }
    if style["sprite"].is_null() {
        style["sprite"] = json!(urls.sprite(SPRITE));
    }
    if style["name"].is_null() {
        style["name"] = json!(name);
    }
    serde_json::to_vec(&style).expect("JSON values serialize")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(tmp: &Path) -> Store {
        Store::new(tmp, true)
    }

    const STYLE: &str = r#"{"version":8,"layers":[{"id":"sea","type":"background"},{"id":"water","type":"fill","source":"protomaps","source-layer":"water"}]}"#;

    #[test]
    fn saves_lists_replaces_and_deletes() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        assert!(s.list().is_empty());
        assert!(s.save("b", STYLE.as_bytes()).unwrap().is_ok());
        assert!(s.save("a-1", STYLE.as_bytes()).unwrap().is_ok());
        assert_eq!(s.list(), ["a-1", "b"]);
        assert!(s.save("b", br#"{"version":8,"layers":[]}"#).unwrap().is_ok());
        assert_eq!(s.read("b").unwrap()["layers"], json!([]));
        assert!(s.delete("b").unwrap());
        assert!(!s.delete("b").unwrap());
        assert_eq!(s.read("b"), None);
        // A save leaves nothing behind but the style.
        assert_eq!(fs::read_dir(tmp.path().join("tiles-styles")).unwrap().count(), 1);
    }

    #[test]
    fn names_never_leave_the_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        fs::write(tmp.path().join("secret.json"), STYLE).unwrap();
        let long = "x".repeat(65);
        for name in ["", "../secret", "a/b", "a\\b", ".hidden", "-a", "A", "a.b", "🦎", "a\u{0}", long.as_str()] {
            assert!(matches!(s.save(name, STYLE.as_bytes()).unwrap(), Err(Refused::Invalid(_))), "{name:?}");
            assert_eq!(s.read(name), None, "{name:?}");
            assert!(!s.delete(name).unwrap(), "{name:?}");
        }
        assert!(tmp.path().join("secret.json").exists());
    }

    #[test]
    fn what_a_client_could_not_load_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        let big = format!(r#"{{"version":8,"layers":[],"name":"{}"}}"#, "x".repeat(MAX_BYTES));
        for (body, why) in [
            ("{\"not\": [\"closed\"", "JSON"),
            ("[]", "object"),
            (r#"{"layers":[]}"#, "version"),
            (r#"{"version":7,"layers":[]}"#, "version"),
            (r#"{"version":8}"#, "layers"),
            (r#"{"version":8,"sources":[],"layers":[]}"#, "sources"),
            (r#"{"version":8,"glyphs":1,"layers":[]}"#, "glyphs"),
            (r#"{"version":8,"sprite":1,"layers":[]}"#, "sprite"),
            (r#"{"version":8,"layers":[{"type":"background"}]}"#, "id"),
            (r#"{"version":8,"layers":[{"id":"a","type":"background"},{"id":"a","type":"background"}]}"#, "two layers"),
            (r#"{"version":8,"layers":[{"id":"a","type":"neon"}]}"#, "neon"),
            (r#"{"version":8,"layers":[{"id":"a","type":"fill"}]}"#, "source"),
            (r#"{"version":8,"layers":[{"id":"a","type":"fill","source":"other"}]}"#, "other"),
            (big.as_str(), "bytes"),
        ] {
            let Err(Refused::Invalid(message)) = s.save("a", body.as_bytes()).unwrap() else { panic!("{why}") };
            assert!(message.contains(why), "{why}: {message}");
        }
        assert!(s.list().is_empty());
        assert!(s.save("a", br#"{"version":8,"sources":{"other":{"type":"vector"}},"layers":[{"id":"a","type":"fill","source":"other"}]}"#).unwrap().is_ok());
    }

    #[test]
    fn the_store_has_a_limit_but_a_style_can_always_be_replaced() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        for i in 0..MAX_STYLES {
            s.save(&format!("s{i}"), STYLE.as_bytes()).unwrap().unwrap();
        }
        assert_eq!(s.save("one-more", STYLE.as_bytes()).unwrap(), Err(Refused::Full));
        assert!(s.save("s0", STYLE.as_bytes()).unwrap().is_ok());
    }

    #[test]
    fn the_tiles_are_those_of_the_bundle_and_the_rest_may_be_its_own() {
        let urls = Urls { url: "https://maps.example.com/tiles", version: "v1", tile_type: "mlt", key: None };
        let composed = |style: Value| serde_json::from_slice::<Value>(&compose(style, "mine", &urls)).unwrap();
        let s = composed(json!({ "version": 8, "sources": { "protomaps": { "type": "vector", "url": "https://elsewhere/" } }, "layers": [] }));
        assert_eq!(s["sources"]["protomaps"], json!({ "type": "vector", "url": "https://maps.example.com/tiles/tilejson.json", "encoding": "mlt" }));
        assert_eq!(s["glyphs"], "https://maps.example.com/tiles/fonts/{fontstack}/{range}.pbf?v=v1");
        assert_eq!(s["sprite"], "https://maps.example.com/tiles/sprites/light?v=v1");
        assert_eq!(s["name"], "mine");
        let s = composed(json!({ "version": 8, "name": "Mine", "glyphs": "https://f/{fontstack}/{range}", "sprite": "https://s/x", "sources": { "other": {} }, "layers": [] }));
        assert_eq!((&s["glyphs"], &s["sprite"], &s["name"]), (&json!("https://f/{fontstack}/{range}"), &json!("https://s/x"), &json!("Mine")));
        assert_eq!(s["sources"]["other"], json!({}));
    }
}
