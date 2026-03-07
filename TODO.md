# TODO

## GSD Runner Architecture

- [ ] **Restructure GSD for testability.** Currently the runner has no inner, easily testable core like agent_pool does. The entire runner is tightly coupled to IPC and external processes, making unit testing impossible. Need to extract a pure-logic core that can be tested without IPC.

  **Problem:** Tests require full IPC setup (daemon, agents, file watchers). Can't test finally tracking logic, retry logic, or task scheduling in isolation.

  **Solution:** Extract a `RunnerCore` or similar that:
  - Takes events (task completed, task failed, response received)
  - Returns effects (queue task, run finally hook, notify origin)
  - Is completely synchronous and testable
  - The outer `TaskRunner` becomes a thin shell that handles IPC and calls into the core

  See agent_pool's architecture: `DaemonCore` handles pure logic, `Daemon` handles IO.

## Agent Pool

- [ ] Kicked message should not include a response_file (it's not needed)
- [ ] When get_task is ctrl+c'd, clean up and notify the pool so the task can be immediately reassigned
