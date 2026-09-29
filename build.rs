//! Embeds the built WebUI (`webui/dist/**`) into the binary.
//!
//! The generated `$OUT_DIR/webui_assets.rs` holds one `(path, content type,
//! bytes)` entry per file. Without a built `webui/dist` (for example a plain
//! `cargo check` on a machine without Node), a small placeholder page is
//! embedded instead, so the crate still builds.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("webmanifest") => "application/manifest+json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect_files(&path, out);
        } else if path.is_file() {
            out.push(path);
        }
    }
}

const PLACEHOLDER: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>Xiao Console</title></head><body style=\"font-family:system-ui,sans-serif;max-width:40rem;margin:4rem auto;padding:0 1rem;line-height:1.5\"><h1>Xiao Console</h1><p>This binary was built without the WebUI. Build it with <code>npm ci &amp;&amp; npm run build</code> in <code>webui/</code>, then build xiao again.</p><p>Binary ini dibuild tanpa WebUI. Jalankan <code>npm ci &amp;&amp; npm run build</code> di <code>webui/</code>, lalu build xiao lagi.</p></body></html>";

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let dist = manifest_dir.join("webui").join("dist");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=webui/dist");

    let mut files = Vec::new();
    if dist.join("index.html").is_file() {
        collect_files(&dist, &mut files);
    }

    let mut code = String::from(
        "/// Embedded WebUI files: (path relative to `webui/dist`, content type, bytes).\n\
         pub(crate) static ASSETS: &[(&str, &str, &[u8])] = &[\n",
    );
    for file in &files {
        let Ok(relative) = file.strip_prefix(&dist) else {
            continue;
        };
        let web_path = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let absolute = file.to_string_lossy().into_owned();
        let _ = writeln!(
            code,
            "    ({web_path:?}, {:?}, include_bytes!({absolute:?})),",
            content_type(file)
        );
    }
    if files.is_empty() {
        let _ = writeln!(
            code,
            "    (\"index.html\", \"text/html; charset=utf-8\", {PLACEHOLDER:?}.as_bytes()),"
        );
    }
    code.push_str("];\n");
    let _ = writeln!(
        code,
        "/// Whether a real WebUI build was embedded.\npub(crate) const WEBUI_BUILT: bool = {};",
        !files.is_empty()
    );

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap_or_default());
    if let Err(error) = std::fs::write(out_dir.join("webui_assets.rs"), code) {
        panic!("cannot write webui_assets.rs: {error}");
    }
}
