//! Agent pool CLI library.
//!
//! Exports the `AgentPoolCli` type for use with `cli_invoker`.

use cli_invoker::InvokableCli;

/// Configuration for invoking the `agent_pool` CLI.
pub struct AgentPoolCli;

impl InvokableCli for AgentPoolCli {
    const NPM_PACKAGE: &'static str = "@gsd-now/agent-pool";
    const BINARY_NAME: &'static str = "agent_pool";
    const CARGO_PACKAGE: &'static str = "agent_pool_cli";
    const ENV_VAR_BINARY: &'static str = "AGENT_POOL";
    const ENV_VAR_COMMAND: &'static str = "AGENT_POOL_COMMAND";
}
