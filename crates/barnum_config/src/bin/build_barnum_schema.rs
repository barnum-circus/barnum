//! Build-time schema generator for Barnum config.
//!
//! Generates both JSON Schema and Zod TypeScript schema files in
//! libs/barnum/ for inclusion in the npm package.
//!
//! Run with: `cargo run -p barnum_config --bin build_barnum_schema`

#![expect(clippy::print_stdout)]
#![expect(clippy::print_stderr)]

use barnum_config::{config_schema, zod::emit_zod};
use std::fs;
use std::path::Path;

fn main() {
    let root = config_schema();

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let Some(crates_dir) = manifest_dir.parent() else {
        eprintln!("Cannot find parent of manifest dir");
        std::process::exit(1);
    };
    let Some(workspace_root) = crates_dir.parent() else {
        eprintln!("Cannot find workspace root");
        std::process::exit(1);
    };
    let libs = workspace_root.join("libs/barnum");

    // JSON Schema
    let json = serde_json::to_string_pretty(&root).unwrap_or_else(|e| {
        eprintln!("Failed to serialize JSON schema: {e}");
        std::process::exit(1);
    });
    write_file(&libs.join("barnum-config-schema.json"), &json);

    // Zod TypeScript schema
    let zod = emit_zod(&root);
    write_file(&libs.join("barnum-config-schema.zod.ts"), &zod);
}

fn write_file(path: &Path, content: &str) {
    if let Err(e) = fs::write(path, content) {
        eprintln!("Failed to write {}: {e}", path.display());
        std::process::exit(1);
    }
    println!("Written: {}", path.display());
}
