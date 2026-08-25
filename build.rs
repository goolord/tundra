use std::fs;
use std::path::{Path, PathBuf};

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

fn main() {
    println!("cargo:rerun-if-changed=resources/models");

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = manifest.join("resources/models");
    if !src.is_dir() {
        return;
    }

    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| manifest.join("target"));
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".into());
    let dest = target_dir.join(profile).join("models");

    if let Err(err) = copy_dir_all(&src, &dest) {
        println!("cargo:warning=Failed to copy bundled models to {}: {err}", dest.display());
    }
}
