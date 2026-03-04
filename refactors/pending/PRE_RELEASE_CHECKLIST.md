# Pre-Release Checklist

Items that must be completed before shipping v0.2.

## Summary (Remaining)

| Item | Doc | Status |
|------|-----|--------|
| Config Schema | CONFIG_SCHEMA_SUBCOMMAND.md | CLI done, schema stub |
| Default Step | DEFAULT_STEP.md | Pending approval |
| State Persistence | STATE_PERSISTENCE.md | Pending approval |
| Documentation | DOCUMENTATION.md | Pending approval |

---

## Must Have (Incomplete)

### 1. Config Schema Subcommand
**Doc:** `CONFIG_SCHEMA_SUBCOMMAND.md`

Add `gsd config schema` subcommand that prints JSON schema. Enables validation and IDE autocomplete.

**Status:** CLI restructure done (`gsd config docs/validate/graph/schema`). Schema stub added. Need to implement actual schema generation with schemars.

---

### 2. Default Step
**Doc:** `DEFAULT_STEP.md`

Allow configs to specify a default starting step so users don't have to pass initial tasks.

**Status:** Document exists, awaiting approval.

---

### 3. State Persistence and Resume
**Doc:** `STATE_PERSISTENCE.md`

Write task queue state to a file so runs can be resumed after interruption.

**Status:** Document created, awaiting approval.

---

### 4. Documentation
**Doc:** `DOCUMENTATION.md`

- README with quick start
- Config file format documentation
- Protocol documentation for agents
- Examples for common use cases

**Status:** Document created, awaiting approval.

---

## Must Have (Complete)

### 5. Cancellable Wait For Task
**Doc:** `CANCELLABLE_WAIT_FOR_TASK.md`

Use crossbeam select! to make blocking operations cancellable. Foundation for graceful shutdown.

**Status:** DONE. WaitError enum, stop detection in VerifiedWatcher, merged to master.

---

### 6. Task Progress Display

When running GSD, show progress: "X task(s) completed, Y task(s) remaining"

**Status:** DONE. Logs after each task completion. Merged to master.

---

### 7. Package Manager Auto-Detection
**Doc:** `AGENT_POOL_COMMAND.md` (in past/)

Auto-detect pnpm/yarn/npm from package.json and use appropriate dlx command.

**Status:** DONE.

---

### 8. Pool Root Configuration for GSD
**Doc:** `GSD_POOL_ROOT.md` (in past/)

Allow passing `--pool-root` to gsd CLI.

**Status:** DONE.

---

### 9. Version Subcommand
**Doc:** `VERSION_SUBCOMMAND.md` (in past/)

Add `version` subcommand.

**Status:** DONE.

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
