//! Writes the preset library to a directory as `<name>.brightfx.json`.
//! Run: `cargo run -p brightfx-tracks --example gen_presets -- ../presets`
//! (from `core/`; from the repo root the path is `presets`).

use brightfx_tracks::presets::{library, render_json};
use std::collections::BTreeSet;
use std::path::Path;

fn main() {
    let dir = std::env::args().nth(1).expect("usage: gen_presets <dir>");
    let dir = Path::new(&dir);
    std::fs::create_dir_all(dir).unwrap();

    let names: BTreeSet<&str> = library().iter().map(|(name, _)| *name).collect();

    // Remove any preset the library no longer names, so the directory
    // never accumulates files for presets that were renamed or deleted.
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else { continue };
        let Some(stem) = file_name.strip_suffix(".brightfx.json") else { continue };
        if !names.contains(stem) {
            let path = entry.path();
            std::fs::remove_file(&path).unwrap();
            println!("removed {}", path.display());
        }
    }

    for (name, config) in library() {
        let path = dir.join(format!("{name}.brightfx.json"));
        std::fs::write(&path, render_json(&config)).unwrap();
        println!("wrote {}", path.display());
    }
}
