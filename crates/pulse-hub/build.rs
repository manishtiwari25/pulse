//! Declares the embedded console bundle (`web/hub/out`) as a build input.
//!
//! `server::Dist` embeds that folder through `rust_embed`, which reads the
//! files at compile time but leaves cargo with no declared dependency on
//! them. Under a shared `CARGO_TARGET_DIR` a cached build could then keep a
//! stale bundle (or none at all) and `server::tests::embedded_export` would
//! fail spuriously. Walking the folder and emitting `rerun-if-changed` for
//! every entry makes cargo rebuild the crate whenever its contents differ.

use std::fs;
use std::path::Path;

fn watch(path: &Path) {
    println!("cargo:rerun-if-changed={}", path.display());
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let child = entry.path();
        if child.is_dir() {
            watch(&child);
        } else {
            println!("cargo:rerun-if-changed={}", child.display());
        }
    }
}

fn main() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = manifest.join("../../web/hub/out");
    // Watch the folder itself even when it does not exist yet, so creating it
    // (e.g. after `npm run build`) triggers a rebuild too.
    watch(&out);
}
