//! Build-time schema generator for Barnum.
//!
//! Generates all schema files in libs/barnum/ for inclusion in the npm package:
//! - barnum-config-schema.json (JSON Schema for config)
//! - barnum-config-schema.zod.ts (Zod schema for config)
//! - barnum-cli-schema.zod.ts (Zod schema for CLI types)
//!
//! Run with: `cargo run -p barnum_cli --bin build_schemas`

#![expect(clippy::print_stdout)]
#![expect(clippy::print_stderr)]

use barnum_cli::Cli;
use barnum_config::{config_schema, zod::emit_zod};
use std::fs;
use std::path::Path;

fn main() {
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

    // Config: JSON Schema
    let config_root = config_schema();
    let json = serde_json::to_string_pretty(&config_root).unwrap_or_else(|e| {
        eprintln!("Failed to serialize JSON schema: {e}");
        std::process::exit(1);
    });
    write_file(&libs.join("barnum-config-schema.json"), &json);

    // Config: Zod TypeScript schema
    let mut config_zod = emit_zod(&config_root);
    config_zod.push_str(
        "\nexport function defineConfig(config: z.input<typeof configFileSchema>): ConfigFile {\n  \
         return configFileSchema.parse(config);\n}\n",
    );
    write_file(&libs.join("barnum-config-schema.zod.ts"), &config_zod);

    // CLI: Zod TypeScript schema
    let cli_root = schemars::schema_for!(Cli);
    let cli_zod = emit_zod(&cli_root);
    write_file(&libs.join("barnum-cli-schema.zod.ts"), &cli_zod);
}

fn write_file(path: &Path, content: &str) {
    if let Err(e) = fs::write(path, content) {
        eprintln!("Failed to write {}: {e}", path.display());
        std::process::exit(1);
    }
    println!("Written: {}", path.display());
}
