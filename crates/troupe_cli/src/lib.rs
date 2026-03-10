//! Troupe CLI library.
//!
//! Exports the `TroupeCli` type for use with `cli_invoker`.

use cli_invoker::InvokableCli;

/// Configuration for invoking the `troupe` CLI.
pub struct TroupeCli;

impl InvokableCli for TroupeCli {
    const NPM_PACKAGE: &'static str = "@barnum/troupe";
    const BINARY_NAME: &'static str = "troupe";
    const CARGO_PACKAGE: &'static str = "troupe_cli";
    const ENV_VAR_BINARY: &'static str = "TROUPE";
    const ENV_VAR_COMMAND: &'static str = "TROUPE_COMMAND";
}
