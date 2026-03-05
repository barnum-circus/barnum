//! Build-time schema generator for GSD config.
//!
//! Generates JSON schema and writes it to libs/gsd/gsd-config-schema.json
//! for inclusion in the npm package.
//!
//! Run with: `cargo run -p gsd_config --bin build_gsd_schema`

#![expect(clippy::print_stdout)]
#![expect(clippy::print_stderr)]

use gsd_config::config_schema;
use std::fs;
use std::path::Path;

fn main() {
    let schema = config_schema();
    let json = serde_json::to_string_pretty(&schema).unwrap_or_else(|e| {
        eprintln!("Failed to serialize schema: {e}");
        std::process::exit(1);
    });

    // Find the output path relative to workspace root
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let Some(crates_dir) = manifest_dir.parent() else {
        eprintln!("Cannot find parent of manifest dir");
        std::process::exit(1);
    };
    let Some(workspace_root) = crates_dir.parent() else {
        eprintln!("Cannot find workspace root");
        std::process::exit(1);
    };

    let output_path = workspace_root.join("libs/gsd/gsd-config-schema.json");

    if let Err(e) = fs::write(&output_path, &json) {
        eprintln!("Failed to write schema file: {e}");
        std::process::exit(1);
    }

    println!("Schema written to: {}", output_path.display());
}
