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

fn write_embedded_resources(manifest: &Path) -> std::io::Result<()> {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let icons_src = manifest.join("resources");
    let mut body = String::from(
        "use iced::widget::svg::Handle;\n\npub fn handle(name: &str) -> Option<Handle> {\n    let bytes: &[u8] = match name {\n",
    );
    let mut embedded = 0usize;
    for name in RESOURCE_FILES {
        let src = icons_src.join(name);
        if src.is_file() {
            warn_if_lfs_pointer(&src, name);
            body.push_str(&format!(
                "        {name:?} => include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/resources/{name}\")),\n"
            ));
            embedded += 1;
        }
    }
    if embedded == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no SVG resources under resources/; run git lfs pull or cargo xtask setup",
        ));
    }
    if !icons_src.join("play.svg").is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "missing resources/play.svg (required fallback icon)",
        ));
    }
    body.push_str("        _ => return None,\n    };\n    Some(Handle::from_memory(bytes))\n}\n");
    fs::write(out_dir.join("embedded_resources.rs"), body)
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

fn is_release_fast_build() -> bool {
    std::env::var("OUT_DIR")
        .ok()
        .is_some_and(|out| out.replace('\\', "/").contains("/release-fast/"))
}

fn require_windows_release_static_crt() {
    if std::env::var("CARGO_CFG_WINDOWS").is_err() {
        return;
    }
    if is_release_fast_build() {
        return;
    }
    if std::env::var("PROFILE").ok().as_deref() != Some("release") {
        return;
    }
    let features = std::env::var("CARGO_CFG_TARGET_FEATURE").unwrap_or_default();
    if features.split(',').any(|feature| feature.trim() == "crt-static") {
        return;
    }
    panic!(
        "Windows release builds must link the static CRT. Set RUSTFLAGS=-C target-feature=+crt-static \
         or use `cargo xtask build --release`."
    );
}

fn warn_if_lfs_pointer(path: &Path, name: &str) {
    let Ok(bytes) = fs::read(path) else {
        return;
    };
    if bytes.starts_with(b"version https://git-lfs.github.com") {
        println!(
            "cargo:warning=Resource {name} is a Git LFS pointer; run `git lfs pull` or `cargo xtask setup`"
        );
    }
}

fn main() {
    require_windows_release_static_crt();
    println!("cargo:rerun-if-changed=resources");
    println!("cargo:rerun-if-changed=resources/models");
    println!("cargo:rerun-if-changed=scripts");

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let models_src = manifest.join("resources/models");
    let scripts_src = manifest.join("scripts");

    write_embedded_resources(&manifest).unwrap_or_else(|err| {
        panic!("Failed to embed SVG resources: {err}");
    });

    for dest in exe_dirs(&manifest) {
        if models_src.is_dir() {
            if let Err(err) = copy_dir_all(&models_src, &dest.join("models")) {
                println!(
                    "cargo:warning=Failed to copy bundled models to {}: {err}",
                    dest.join("models").display()
                );
            }
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
