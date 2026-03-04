# Pre-Release Checklist

Items that must be completed before shipping v0.2.

## Summary

| Item | Doc | Status |
|------|-----|--------|
| Cancellable Wait | CANCELLABLE_WAIT_FOR_TASK.md | In progress |
| Task Progress Display | (needs creation) | Not started |
| Default Step | DEFAULT_STEP.md | Pending approval |
| Config Schema | CONFIG_SCHEMA_SUBCOMMAND.md | Pending approval |
| State Persistence | STATE_PERSISTENCE.md | Pending approval |
| Documentation | DOCUMENTATION.md | Pending approval |
| Package Manager Auto-Detection | past/AGENT_POOL_COMMAND.md | DONE |
| Pool Root for GSD | past/GSD_POOL_ROOT.md | DONE |
| Version Subcommand | past/VERSION_SUBCOMMAND.md | DONE |

---

## Must Have (Incomplete)

### 1. Cancellable Wait For Task
**Doc:** `CANCELLABLE_WAIT_FOR_TASK.md`

Use crossbeam select! to make blocking operations cancellable. Foundation for graceful shutdown.

**Status:** In progress on `stop-file-cancellation` branch. CI passing.

---

### 2. Task Progress Display
**Doc:** (needs creation)

When running GSD, show progress visualization - at minimum a count of remaining/completed tasks. Users need feedback that work is happening.

Options:
- Simple: `[3/10] Processing step_name...`
- Progress bar: `[████░░░░░░] 3/10 tasks`
- Periodic summary: Print task counts every N seconds

**Status:** Not started.

---

### 3. Default Step
**Doc:** `DEFAULT_STEP.md`

Allow configs to specify a default starting step so users don't have to pass initial tasks.

**Status:** Document exists, awaiting approval.

---

### 4. Config Schema Subcommand
**Doc:** `CONFIG_SCHEMA_SUBCOMMAND.md`

Add `gsd schema` and `gsd config` subcommands that print JSON schemas. Enables validation and IDE autocomplete.

**Status:** Document updated, awaiting approval.

---

### 5. State Persistence and Resume
**Doc:** `STATE_PERSISTENCE.md`

Write task queue state to a file so runs can be resumed after interruption.

**Status:** Document created, awaiting approval.

---

### 6. Documentation
**Doc:** `DOCUMENTATION.md`

- README with quick start
- Config file format documentation
- Protocol documentation for agents
- Examples for common use cases

**Status:** Document created, awaiting approval.

---

## Must Have (Complete)

### 7. Package Manager Auto-Detection
**Doc:** `AGENT_POOL_COMMAND.md` (in past/)

Auto-detect pnpm/yarn/npm from package.json and use appropriate dlx command. Zero config for package manager users.

**Status:** DONE. CLI invoker with package manager detection merged.

---

### 8. Pool Root Configuration for GSD
**Doc:** `GSD_POOL_ROOT.md` (in past/)

Allow passing `--pool-root` to gsd CLI so users can specify where pools live.

**Status:** DONE. `--pool-root` global flag added to gsd CLI.

---

### 9. Version Subcommand
**Doc:** `VERSION_SUBCOMMAND.md` (in past/)

Add `version` subcommand. Generate version.txt during CI. Ensure gsd uses matching agent_pool version when using dlx.

**Status:** DONE. Version subcommand with --json flag works. CI generates version.txt.

---

## Nice to Have (Post-Release)

- Windows support for package manager detection
- Sync testing harness improvements (`SYNC_TESTING_HARNESS.md`)

---

## Completion Criteria

All "Must Have" items must be:
1. Documented in refactors/pending/ (or past/ if done)
2. Approved by user
3. Implemented and tested
4. Merged to master
5. CI passing

Then we can tag v0.2 and publish to npm with `latest` tag.
