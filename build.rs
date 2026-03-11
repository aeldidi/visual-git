use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=ui-dist");

    let out_dir = PathBuf::from(
        env::var("OUT_DIR").expect("cargo did not provide OUT_DIR"),
    );
    let generated_path = out_dir.join("embedded_assets.rs");
    let profile = env::var("PROFILE").unwrap_or_default();

    let ui_dist = PathBuf::from("ui-dist");
    let mut assets = Vec::<(String, String)>::new();

    if ui_dist.is_dir() {
        collect_assets(&ui_dist, &ui_dist, &mut assets)
            .expect("failed to walk ui-dist");
        assets.sort_by(|a, b| a.0.cmp(&b.0));
    }

    let has_index =
        assets.iter().any(|(web_path, _)| web_path == "/index.html");
    if profile == "release" && !has_index {
        panic!(
            "release build requires frontend assets in ui-dist/index.html; run `npm run ui:build` before `cargo build --release`"
        );
    }

    let mut generated = String::new();
    generated.push_str("pub fn get(path: &str) -> Option<&'static [u8]> {\n");
    generated.push_str("    match path {\n");
    for (web_path, disk_path) in &assets {
        generated.push_str("        ");
        generated.push_str(&to_rust_string(web_path));
        generated.push_str(" => Some(include_bytes!(");
        generated.push_str(&to_rust_string(disk_path));
        generated.push_str(")),\n");
    }
    generated.push_str("        _ => None,\n");
    generated.push_str("    }\n");
    generated.push_str("}\n\n");
    generated.push_str(&format!(
        "pub const ASSET_COUNT: usize = {};\n",
        assets.len()
    ));

    fs::write(&generated_path, generated).unwrap_or_else(|err| {
        panic!(
            "failed to write generated assets file at {}: {}",
            generated_path.display(),
            err
        )
    });
}

fn collect_assets(
    base_dir: &Path,
    current_dir: &Path,
    out: &mut Vec<(String, String)>,
) -> Result<(), String> {
    let dir_entries = fs::read_dir(current_dir).map_err(|err| {
        format!("read_dir({}): {}", current_dir.display(), err)
    })?;

    for entry in dir_entries {
        let entry =
            entry.map_err(|err| format!("read_dir entry error: {}", err))?;
        let path = entry.path();

        if path.is_dir() {
            collect_assets(base_dir, &path, out)?;
            continue;
        }

        if !path.is_file() {
            continue;
        }

        let rel = path
            .strip_prefix(base_dir)
            .map_err(|err| format!("strip_prefix failed: {}", err))?;
        let rel = rel.to_string_lossy().replace('\\', "/");
        let web_path = format!("/{}", rel);
        let disk_path = path
            .canonicalize()
            .unwrap_or_else(|_| path.clone())
            .to_string_lossy()
            .to_string();
        out.push((web_path, disk_path));
    }

    Ok(())
}

fn to_rust_string(value: &str) -> String {
    format!("{:?}", value)
}
