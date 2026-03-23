//! Build-time schema generator for Barnum CLI types.
//!
//! Generates a Zod TypeScript schema file in libs/barnum/ for the CLI
//! argument types. This enables typed programmatic invocation from Node.
//!
//! Run with: `cargo run -p barnum_cli --bin build_cli_schema`

#![expect(clippy::print_stdout)]
#![expect(clippy::print_stderr)]

use barnum_cli::Cli;
use barnum_config::zod::emit_zod;
use std::fs;
use std::path::Path;

fn main() {
    let root = schemars::schema_for!(Cli);
    let zod = emit_zod(&root);

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

    let path = libs.join("barnum-cli-schema.zod.ts");
    if let Err(e) = fs::write(&path, &zod) {
        eprintln!("Failed to write {}: {e}", path.display());
        std::process::exit(1);
    }
    println!("Written: {}", path.display());
}
