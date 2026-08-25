use std::fs;
use std::path::{Path, PathBuf};

include!("src/resource_files.rs");

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

fn copy_icons(src_dir: &Path, dst_dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst_dir)?;
    for name in RESOURCE_FILES {
        let src = src_dir.join(name);
        if src.is_file() {
            fs::copy(&src, dst_dir.join(name))?;
        }
    }
    Ok(())
}

fn copy_scripts(src_dir: &Path, dst_dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst_dir)?;
    for entry in fs::read_dir(src_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.ends_with(".py") {
            fs::copy(entry.path(), dst_dir.join(name))?;
        }
    }
    Ok(())
}

fn exe_dirs(manifest: &Path) -> Vec<PathBuf> {
    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| manifest.join("target"));
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".into());
    let mut dirs = vec![target_dir.join(&profile)];
    if let Ok(triple) = std::env::var("TARGET") {
        let triple_dir = target_dir.join(triple).join(&profile);
        if !dirs.contains(&triple_dir) {
            dirs.push(triple_dir);
        }
    }
    dirs
}

fn main() {
    println!("cargo:rerun-if-changed=resources");
    println!("cargo:rerun-if-changed=resources/models");
    println!("cargo:rerun-if-changed=scripts");

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let models_src = manifest.join("resources/models");
    let icons_src = manifest.join("resources");
    let scripts_src = manifest.join("scripts");

    for dest in exe_dirs(&manifest) {
        if models_src.is_dir() {
            if let Err(err) = copy_dir_all(&models_src, &dest.join("models")) {
                println!(
                    "cargo:warning=Failed to copy bundled models to {}: {err}",
                    dest.join("models").display()
                );
            }
        }
        if let Err(err) = copy_icons(&icons_src, &dest.join("resources")) {
            println!(
                "cargo:warning=Failed to copy icons to {}: {err}",
                dest.join("resources").display()
            );
        }
        if scripts_src.is_dir() {
            if let Err(err) = copy_scripts(&scripts_src, &dest.join("scripts")) {
                println!(
                    "cargo:warning=Failed to copy scripts to {}: {err}",
                    dest.join("scripts").display()
                );
            }
        }
    }
}
