use std::fs;
use std::path::PathBuf;

/// Generates `embedded_resources.rs`, which bakes every `resources/*.svg` icon
/// into the binary. Models and scripts are not copied anywhere: dev builds find
/// them in the source tree, and `cargo xtask package` stages them for release.
fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let resources = manifest.join("resources");
    println!("cargo:rerun-if-changed={}", resources.display());

    let mut names: Vec<String> = fs::read_dir(&resources)
        .expect("read resources/")
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.ends_with(".svg"))
        .collect();
    names.sort();
    assert!(
        !names.is_empty(),
        "no icons in resources/; run `git lfs pull` or `cargo xtask setup`"
    );

    let mut body = String::from(
        "use iced::widget::svg::Handle;\n\npub fn handle(name: &str) -> Option<Handle> {\n    let bytes: &[u8] = match name {\n",
    );
    for name in &names {
        let src = resources.join(name);
        println!("cargo:rerun-if-changed={}", src.display());
        if fs::read(&src).is_ok_and(|bytes| bytes.starts_with(b"version https://git-lfs.github.com")) {
            println!("cargo:warning=Resource {name} is a Git LFS pointer; run `git lfs pull` or `cargo xtask setup`");
        }
        body.push_str(&format!(
            "        {name:?} => include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/resources/{name}\")),\n"
        ));
    }
    body.push_str("        _ => return None,\n    };\n    Some(Handle::from_memory(bytes))\n}\n");

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("embedded_resources.rs");
    if fs::read_to_string(&out).ok().as_deref() != Some(body.as_str()) {
        fs::write(&out, body).expect("write embedded_resources.rs");
    }
}
