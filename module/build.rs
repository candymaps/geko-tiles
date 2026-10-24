//! The Planetiler profile goes into the library of the module: `GEKO_TILES_PROFILE`, the jar that
//! `ci/profile.sh` builds, which `ci/modules.sh` passes. Without it the module still compiles (for
//! the checks and the tests), and its build says that it has no profile.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=GEKO_TILES_PROFILE");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("set by cargo")).join("protomaps-basemap.jar");
    match std::env::var_os("GEKO_TILES_PROFILE") {
        Some(jar) => {
            let jar = PathBuf::from(jar);
            println!("cargo:rerun-if-changed={}", jar.display());
            std::fs::copy(&jar, &out).unwrap_or_else(|e| panic!("GEKO_TILES_PROFILE={}: {e}", jar.display()));
        }
        None => std::fs::write(&out, b"").expect("OUT_DIR is writable"),
    }
}
