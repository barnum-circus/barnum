//! Agent pool CLI library.
//!
//! Exports the `AgentPoolCli` type for use with `cli_invoker`.

use cli_invoker::InvokableCli;

/// Configuration for invoking the `agent_pool` CLI.
pub struct AgentPoolCli;

impl InvokableCli for AgentPoolCli {
    // TODO: Using @main is a workaround because `latest` (0.1.0) was published without binaries.
    // The real fix is for gsd and agent-pool to use the same version (e.g., gsd invokes
    // agent-pool at its own version, not whatever `latest` or `main` happens to be).
    const NPM_PACKAGE: &'static str = "@gsd-now/agent-pool@main";
    const BINARY_NAME: &'static str = "agent_pool";
    const CARGO_PACKAGE: &'static str = "agent_pool_cli";
    const ENV_VAR_BINARY: &'static str = "AGENT_POOL";
    const ENV_VAR_COMMAND: &'static str = "AGENT_POOL_COMMAND";
}
