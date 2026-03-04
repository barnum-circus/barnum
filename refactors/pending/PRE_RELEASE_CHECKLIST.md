# Pre-Release Checklist

Items that must be completed before shipping v0.2.

## Must Have

### 1. Package Manager Auto-Detection
**Doc:** `AGENT_POOL_COMMAND.md` (in past/)

Auto-detect pnpm/yarn/npm from package.json and use appropriate dlx command. Zero config for package manager users.

**Status:** IMPLEMENTED. CLI invoker with package manager detection merged.

---

### 2. Pool Root Configuration for GSD
**Doc:** `GSD_POOL_ROOT.md` (in past/)

Allow passing `--pool-root` to gsd CLI so users can specify where pools live.

**Status:** IMPLEMENTED. `--pool-root` global flag added to gsd CLI.

---

### 3. Version Subcommand
**Doc:** `VERSION_SUBCOMMAND.md` (in past/)

Add `version` subcommand. Generate version.txt during CI. Ensure gsd uses matching agent_pool version when using dlx.

**Status:** IMPLEMENTED. Version subcommand with --json flag works. CI generates version.txt.

---

### 4. Cancellable Wait For Task
**Doc:** `CANCELLABLE_WAIT_FOR_TASK.md`

Use crossbeam select! to make blocking operations cancellable. Foundation for graceful shutdown.

**Status:** Document created, crossbeam migration complete. Awaiting approval for cancellation work.

---

### 5. Default Step
**Doc:** `DEFAULT_STEP.md`

Allow configs to specify a default starting step so users don't have to pass initial tasks.

**Status:** Document exists, awaiting approval.

---

### 6. Config Schema Subcommand
**Doc:** `CONFIG_SCHEMA_SUBCOMMAND.md`

Add `gsd schema` subcommand that prints the JSON schema of the config format. Enables validation and IDE autocomplete.

**Status:** Document created, awaiting approval.

---

### 7. State Persistence and Resume
**Doc:** `STATE_PERSISTENCE.md`

Write task queue state to a file so runs can be resumed after interruption.

**Status:** Document created, awaiting approval.

---

### 8. Documentation
**Doc:** `DOCUMENTATION.md`

- README with quick start
- Config file format documentation
- Protocol documentation for agents
- Examples for common use cases

**Status:** Document created, awaiting approval.

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

---

## Summary

| Item | Doc | Status |
|------|-----|--------|
| Package Manager Auto-Detection | past/AGENT_POOL_COMMAND.md | DONE |
| Pool Root for GSD | past/GSD_POOL_ROOT.md | DONE |
| Version Subcommand | past/VERSION_SUBCOMMAND.md | DONE |
| Cancellable Wait | CANCELLABLE_WAIT_FOR_TASK.md | Pending approval |
| Default Step | DEFAULT_STEP.md | Pending approval |
| Config Schema | CONFIG_SCHEMA_SUBCOMMAND.md | Pending approval |
| State Persistence | STATE_PERSISTENCE.md | Pending approval |
| Documentation | DOCUMENTATION.md | Pending approval |
