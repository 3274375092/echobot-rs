//! Build script for `echobot-app`.
//!
//! Copies the Python EchoBot web console's frontend assets
//! (`../../EchoBot/echobot/app/web/`) into the crate's own
//! `web-assets/` directory at build time, so the `include_dir!` macro
//! in `src/create_app.rs` can embed them without relying on a
//! `..`-escape path that include_dir 0.7 mishandles.
//!
//! Source of truth for the assets stays with the Python crate; this
//! script just snapshots the latest tree at build time. If the
//! Python frontend dir is missing, the script leaves the destination
//! untouched and lets the build fail loudly when include_dir! tries
//! to embed an empty directory.

use std::path::PathBuf;
use std::{fs, io};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = manifest_dir
        .join("..")
        .join("..")
        .join("..")
        .join("EchoBot")
        .join("echobot")
        .join("app")
        .join("web");
    let dest = manifest_dir.join("web-assets");

    println!("cargo:rerun-if-changed={}", source.display());

    if !source.is_dir() {
        // No source → leave dest alone; include_dir! will produce an
        // empty bundle and the runtime test will catch it.
        eprintln!(
            "echobot-app build.rs: web source not found at {} — skipping copy",
            source.display()
        );
        return;
    }

    if let Err(e) = copy_tree(&source, &dest) {
        panic!(
            "echobot-app build.rs: failed to copy web assets from {} to {}: {e}",
            source.display(),
            dest.display()
        );
    }
    println!(
        "echobot-app build.rs: copied web assets from {} to {}",
        source.display(),
        dest.display()
    );
}

fn copy_tree(source: &std::path::Path, dest: &std::path::Path) -> io::Result<()> {
    if dest.exists() {
        fs::remove_dir_all(dest)?;
    }
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if file_type.is_dir() {
            copy_tree(&from, &to)?;
        } else if file_type.is_file() {
            fs::copy(&from, &to)?;
        } else {
            // Skip symlinks, sockets, etc.
            eprintln!(
                "echobot-app build.rs: skipping non-regular entry {}",
                from.display()
            );
        }
    }
    Ok(())
}
