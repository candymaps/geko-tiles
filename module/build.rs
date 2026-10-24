//! The Planetiler profile and the layers of the Protomaps styles go into the library of the module:
//! `GEKO_TILES_PROFILE`, the jar that `ci/profile.sh` builds, which `ci/modules.sh` passes, with the
//! layers beside it (`styles/<flavor>/<lang>.json`). The layers are compressed one by one, into
//! `styles.bin`, with their table in `styles.rs`. Without the profile the module still compiles
//! (for the checks and the tests), and its build says that it has no profile and it serves no style.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::GzEncoder;

fn main() {
    println!("cargo:rerun-if-env-changed=GEKO_TILES_PROFILE");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("set by cargo"));
    let profile = std::env::var_os("GEKO_TILES_PROFILE").map(PathBuf::from);
    match &profile {
        Some(jar) => {
            println!("cargo:rerun-if-changed={}", jar.display());
            fs::copy(jar, out.join("protomaps-basemap.jar")).unwrap_or_else(|e| panic!("GEKO_TILES_PROFILE={}: {e}", jar.display()));
        }
        None => fs::write(out.join("protomaps-basemap.jar"), b"").expect("OUT_DIR is writable"),
    }
    let styles = profile.as_deref().and_then(Path::parent).map(|dir| dir.join("styles"));
    if let Some(dir) = &styles {
        println!("cargo:rerun-if-changed={}", dir.display());
    }
    pack_styles(styles.as_deref(), &out);
}

/// `styles.bin`, each layers file compressed with gzip, and `styles.rs`, the table of
/// `(flavor, lang, offset, length)` sorted by flavor and lang.
fn pack_styles(dir: Option<&Path>, out: &Path) {
    let (mut bin, mut table) = (Vec::new(), String::from("pub const STYLES: &[(&str, &str, usize, usize)] = &[\n"));
    if let Some(dir) = dir {
        let sorted = |dir: &Path| {
            let mut entries: Vec<PathBuf> = fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())).map(|e| e.expect("a readable entry").path()).collect();
            entries.sort();
            entries
        };
        for flavor_dir in sorted(dir) {
            let flavor = flavor_dir.file_name().and_then(|n| n.to_str()).expect("a UTF-8 name").to_owned();
            for file in sorted(&flavor_dir) {
                let Some(lang) = file.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".json")) else { continue };
                let mut gz = GzEncoder::new(Vec::new(), Compression::best());
                gz.write_all(&fs::read(&file).expect("a readable style")).expect("in memory");
                let gz = gz.finish().expect("in memory");
                table.push_str(&format!("    ({flavor:?}, {lang:?}, {}, {}),\n", bin.len(), gz.len()));
                bin.extend_from_slice(&gz);
            }
        }
        assert!(!bin.is_empty(), "no styles in {}: run ci/profile.sh", dir.display());
    }
    table.push_str("];\n");
    fs::write(out.join("styles.bin"), bin).expect("OUT_DIR is writable");
    fs::write(out.join("styles.rs"), table).expect("OUT_DIR is writable");
}
