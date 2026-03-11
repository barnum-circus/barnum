# TODO

*Last updated: 2026-03-07*

## Barnum Runner Architecture

- [ ] **Restructure Barnum for testability.** Currently the runner has no inner, easily testable core like troupe does. The entire runner is tightly coupled to IPC and external processes, making unit testing impossible. Need to extract a pure-logic core that can be tested without IPC.

  **Problem:** Tests require full IPC setup (daemon, agents, file watchers). Can't test finally tracking logic, retry logic, or task scheduling in isolation.

  **Solution:** Extract a `RunnerCore` or similar that:
  - Takes events (task completed, task failed, response received)
  - Returns effects (queue task, run finally hook, notify origin)
  - Is completely synchronous and testable
  - The outer `TaskRunner` becomes a thin shell that handles IPC and calls into the core

  See troupe's architecture: `DaemonCore` handles pure logic, `Daemon` handles IO.

## Config / Schema

- [ ] **Support Pool actions for hooks.** Currently `PreHook`, `PostHook`, and `FinallyHook` are each one-variant enums with only `Command`. Add a `Pool` variant to each so hooks can delegate to AI agents (e.g., a finally hook that summarizes results via an agent).

- [ ] **Add `MaybeLinked` wrapping to hook scripts.** Allow hook scripts to be specified as `{"link": "path/to/script.sh"}` in addition to inline `{"kind": "Command", "script": "..."}`, matching how `instructions` works for Pool actions.

## CI

- [ ] **Deploy website should not depend on Rust jobs.** The `deploy-website` job currently depends on `gate-docs` and `build-website` but the overall CI workflow structure means it can be blocked by unrelated Rust job failures. It should be fully independent of fmt, clippy, test, etc.

- [ ] **Binary cache key should exclude test files.** The build jobs cache binaries with `key: <platform>-${{ hashFiles('Cargo.lock', 'crates/**') }}`. This includes test files (`crates/*/tests/**`), so changing a test file invalidates the cache and triggers a full rebuild even though the binary is identical. The cache key should hash only source files that affect the binary output (e.g., `crates/*/src/**`, `Cargo.lock`, `Cargo.toml`).

## Troupe

- [ ] Kicked message should not include a response_file (it's not needed)
- [ ] When get_task is ctrl+c'd, clean up and notify the pool so the task can be immediately reassigned

## Resilience Tests

Tests to ensure the daemon handles pathological conditions gracefully.

- [ ] **Ready.json flood resilience.** The daemon should handle a flood of ready.json file creations without hanging or excessive memory/CPU usage.

  **Background:** We discovered this issue when the test harness spawned 32 workers, each looping with a 500ms timeout. On each timeout, `wait_for_task()` wrote a NEW ready.json with a NEW UUID. This caused:
  - Thousands of inotify events per second
  - Worker IDs climbing to 1000+ as each new UUID was treated as a new worker
  - Daemon became unresponsive processing the event flood
  - Tests timed out waiting for tasks to be assigned

  **Root cause:** The anonymous worker protocol generates a new UUID on each `wait_for_task()` call. Legitimate use is fine, but rapid retries flood the daemon.

  **Test scenarios:**
  1. Spawn N workers that rapidly create/delete ready.json files (simulating buggy retry logic)
  2. Verify daemon remains responsive to legitimate task submissions
  3. Verify daemon doesn't consume unbounded memory tracking stale worker UUIDs
  4. Verify daemon can recover when the flood stops

  **Potential mitigations to test:**
  - Rate limiting per-directory inotify event processing
  - Expiring stale worker registrations after timeout
  - Debouncing rapid ready.json changes from same path prefix
  - Logging warnings when worker churn exceeds threshold

- [ ] **Slow/hung worker resilience.** Daemon should handle workers that accept tasks but never respond.

- [ ] **Partial file write resilience.** Daemon should handle truncated or malformed JSON in ready.json/response.json files.

- [ ] **Filesystem full resilience.** Daemon should handle ENOSPC gracefully when writing task.json files.
