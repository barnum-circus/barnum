# Pre-Release Checklist

Items that must be completed before shipping v0.2.

## Must Have

### 1. Package Manager Auto-Detection
**Doc:** `AGENT_POOL_COMMAND.md`

Auto-detect pnpm/yarn/npm from package.json and use appropriate dlx command. Zero config for package manager users.

**Status:** Document created, awaiting approval.

---

### 2. Pool Root Configuration for GSD
**Doc:** (needs creation)

Allow passing `--pool-root` to gsd CLI so users can specify where pools live. Currently hardcoded.

**Status:** Not started.

---

### 3. Version Subcommand
**Doc:** `VERSION_SUBCOMMAND.md`

Add `version` subcommand. Generate version.txt during CI. Ensure gsd uses matching agent_pool version when using dlx.

**Status:** IMPLEMENTED. Version subcommand with --json flag works. CI generates version.txt. Matching version for dlx is part of package manager auto-detection (item 1).

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
**Doc:** (needs creation)

Add `gsd schema` subcommand that prints the JSON schema of the config format. This allows users to validate configs and IDE integrations to provide autocomplete.

**Status:** Not started.

---

### 7. State Persistence and Resume
**Doc:** (needs creation)

Write task queue state to a file so runs can be resumed after interruption. Enables:
- `gsd run --state-file state.json` - writes state as it runs
- `gsd resume state.json` - resumes from saved state

**Status:** Not started.

---

### 8. Documentation
**Doc:** (needs creation)

- README with quick start
- Config file format documentation
- Protocol documentation for agents
- Examples for common use cases

**Status:** Not started.

---

## Nice to Have (Post-Release)

- Windows support for package manager detection
- Concurrent task submission fix (`CONCURRENT_FILE_SUBMISSION_FIX.md`)
- Sync testing harness improvements (`SYNC_TESTING_HARNESS.md`)

---

## Completion Criteria

All "Must Have" items must be:
1. Documented in refactors/pending/
2. Approved by user
3. Implemented and tested
4. Merged to master
5. CI passing

Then we can tag v0.2 and publish to npm with `latest` tag.
