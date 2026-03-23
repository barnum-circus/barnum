# Refactor Process

## CRITICAL: Never Start Without Explicit Approval

**DO NOT begin any implementation work without explicit user approval.** This includes:
- Writing code
- Creating branches
- Running tests on proposed changes
- Committing code to master
- "Investigating" by making changes

**Exception: Markdown files are fair game.** Feel free to update documentation, refactor docs, and design documents as we discuss. These changes help capture our thinking and don't affect code behavior. Commit and push markdown changes freely.

When the user describes a task or asks you to design something:
1. Write the plan/design document (markdown - do this immediately)
2. Present it to the user
3. **STOP AND WAIT** for explicit approval ("go ahead", "implement it", "approved")
4. Only then begin code implementation

**Asking "Want me to run this?" is NOT the same as waiting for approval.** Just present what you've designed and wait. The user will tell you when to proceed.

---

## Limit refactor complexity

Each refactor document should cover a single, focused architectural change. If a refactor grows to encompass multiple independent concerns, break it into separate documents that can be reviewed, approved, and implemented independently. A refactor that requires understanding two unrelated subsystems to review is two refactors.

Signs a refactor should be split:
- The document has phases that could land and be validated without the later phases existing
- Two sections of the document modify unrelated subsystems
- The motivation section lists multiple distinct problems
- You find yourself writing "this is addressed in Phase N" to defer complexity within the same document

Each sub-refactor gets its own file in `refactors/pending/`. Use blocking relationships (noted at the top of each document) when one refactor depends on another being completed first.

---

Big refactors follow a two-phase process:

## Phase 1: Architecture document

Create a markdown file in `refactors/pending/` describing:
- Motivation and goals
- Current state (with line numbers and code snippets)
- Proposed changes at the architectural level
- Open questions and design decisions

This document captures the *shape* of the refactor without getting into implementation details.

## STOP: Wait for approval

**After writing the architecture document, STOP and wait for explicit user approval before implementing ANYTHING.**

### What "approval" means

Approval is ONLY one of these explicit statements:
- "Go ahead"
- "Implement it"
- "Approved"
- "Let's do it"
- "Start implementing"

### What is NOT approval

- User asking questions about the document
- User providing feedback or suggestions
- User saying "looks good" (this is feedback, not approval)
- User discussing the approach further
- Silence

### Do NOT:
- Start implementing tasks
- Make "small independent changes"
- Commit code changes (except the refactor document itself)
- Push anything
- Write ANY code

The document exists for the user to review. They may have feedback, want changes to the approach, or decide not to proceed at all. **Only begin implementation after the user explicitly says to proceed.**

### After writing the document

Your response should be something like: "Created the refactor document at `refactors/pending/FOO.md`. Let me know if you'd like any changes or if you're ready to proceed with implementation."

Then STOP. Do not do anything else until the user explicitly approves.

## Document rigor

A refactor document is a technical specification. If the document has a bug, the implementation will have the same bug. Every design choice, code block, and prose description must survive adversarial review before the document is presented.

### Self-review before presenting

After finishing a draft, do a full verification pass. This is not optional and not a skim. For each of the following, trace through the document and confirm it holds:

1. **Every design choice must follow from a stated goal.** If the motivation says "state and log must stay in sync", and the design introduces an Applier trait with a batch API, the document must connect these: batching enables submitted+completed cancellation during resume. If a design choice exists without a reason traceable to the motivation, either the motivation is incomplete or the choice is wrong.

2. **Every code block must be internally consistent with every other code block.** If the Applier trait takes `&[StateLogEntry]` in the trait definition, every impl block must also take `&[StateLogEntry]`. If a struct has a field called `pending_dispatches`, every method that references it must use that name. Cross-reference every code block against every other code block that touches the same types.

3. **Every prose claim must agree with the code.** If the prose says "the event loop receives entries and calls apply on each applier", there must be a code block showing exactly that loop. If the prose says "flush_dispatches is an implementation detail inside apply", then flush_dispatches must not appear in the event loop code. Read each prose paragraph, then find the code block it describes and verify they match.

4. **Architectural boundaries must be consistent throughout.** If the design says the main loop holds a `Vec<Box<dyn Applier>>` and iterates it generically, then no code block should show the main loop accessing a specific applier by type. If a component is described as having no I/O, none of its methods should do I/O. Check every boundary claim against every code block for that component.

5. **Control flow must be complete and traceable.** A reader should be able to start at the entry point and follow execution through every branch. If apply generates finally entries, show where those entries go. If termination depends on channel senders being dropped, show the exact sequence. If step() produces entries before they reach the channel, show where step() is called and how entries get to the channel. No gaps.

6. **Every type and function referenced must be defined.** If a code block calls `interpret_response`, the document must either define it or state that it's existing code with a file reference. If a struct uses `NonZeroU16`, the import context must be clear. Undefined references are bugs.

### Simplification pass

After verifying consistency, re-read the entire document and ask whether the architecture is as simple as it could be. This pass is about reducing the design, not checking it.

For every component, ask: can this be eliminated? If RunState and StateRunner are separate structs, is there a reason they can't be one struct? If they must be separate, the document should make the reason obvious (e.g., RunState is tested in isolation without I/O). If the reason isn't obvious, they should be merged.

For every piece of state, ask: who owns this, and is ownership unambiguous? Every field should live on exactly one struct with a clear reason for being there. If two components both need access to the same data, the document must show how that access works — shared reference, passed as argument, returned as value. "Both components have access to the config" is ambiguous; "StateRunner owns the config and passes `&Step` to RunState methods" is not.

For every interface between components, ask: is this the narrowest possible surface? If a method takes `&Config` but only reads one field, it should take that field. If a trait has three methods but every impl only does meaningful work in one of them, the trait is too wide. Interfaces should be hard to misuse — a caller that compiles should be a caller that's correct.

For every abstraction, ask: does this compose? If you add a third applier, does the architecture accommodate it without changes to the event loop or other appliers? If you swap out the log format, does anything outside LogApplier need to change? Components that require coordinated changes across boundaries are not composable.

If this pass produces simplifications, apply them and redo the consistency checks above. A simpler design with fewer components, narrower interfaces, and unambiguous ownership is always preferred over a sophisticated one.

### Writing quality

Read `.claude/writing.md` before writing prose in a refactor document. After the self-review pass above, do a second pass for writing quality. Common failures in refactor docs:

- Negative parallelism to describe boundaries ("No I/O, no config awareness, no knowledge of finally") — instead, state what the component does and where those responsibilities live.
- Short punchy fragments for drama ("Implements Applier. Owns state, dispatch queue, pool connection.") — write a sentence.
- "Not X. Not Y. Just Z." countdown — rewrite as a direct statement of what changes.
- Describing what things don't do instead of what they do ("The event loop doesn't know about step, doesn't track in_flight") — state the positive: "The event loop receives entries from the channel and calls apply on each applier in the vector."

## Phase 2: Practical task list

Convert the architecture document into concrete, **independently deployable tasks**. Each task should be:

1. **Self-contained** - Can be implemented and deployed without other tasks
2. **Detailed** - Broken into numbered subtasks with specific file locations
3. **Actionable** - Include code snippets showing exactly what changes

**Expected level of detail for tasks:**

```markdown
## Task 1: Add Socket Transport Variant

**Goal:** One sentence describing the outcome.

**Current state:** What exists now and why it's insufficient.

### 1.1: Subtask Name

**File:** `path/to/file.rs`

Description of what to change:

\`\`\`rust
// Before
pub enum Transport {
    Directory(PathBuf),
}

// After
pub enum Transport {
    Directory(PathBuf),
    Socket(Stream),  // NEW
}
\`\`\`

**Complication:** Any gotchas or decisions to make.

### 1.2: Next Subtask
...
```

Each subtask should be small enough that someone could implement it without asking questions. Include:
- Exact file paths
- Before/after code snippets
- Complications or edge cases
- How to test the change

For examples, see `TRANSPORT_ABSTRACTION.md` and `DAEMON_REFACTOR.md` in `refactors/past/`.

## Branching strategy

**One branch per logical change. Keep branches small.** Each branch should do one thing, squash merge to master, and produce one clean commit. If a refactor has three independent steps, that's three branches — not three commits on one branch.

When implementing a refactor:

1. **Break the work into the smallest independent units** — each unit gets its own branch
2. **Each branch should be a single logical change** that passes CI
3. **Squash merge each branch to master** as soon as it's green — don't wait for later branches
4. **Later branches rebase on master** after earlier ones merge

The goal: master gets a stream of small, focused commits. Each one is easy to review, easy to bisect, and easy to revert. Large branches with many commits are a sign that the work wasn't decomposed enough.

### Example: a 3-step refactor

Instead of one branch with 3 commits:
1. `refactor/extract-types` — extract types into lib.rs, merge to master
2. `refactor/add-derives` — add serde/schemars derives, merge to master
3. `refactor/generate-schema` — add schema generator binary, merge to master

Each branch is created from the latest master after the previous one merges. Each produces one squash commit on master.

### What makes a good atomic commit

- Self-contained change that compiles and passes tests
- Does one logical thing (add a type, update a function, remove dead code)
- Commit message explains the "why"
- Can be reverted independently if needed

### Test-first pattern for bug fixes

**When fixing bugs or changing behavior, write the test first.**

The pattern:
1. **First commit: Add test with `#[should_panic]`** - The test demonstrates the bug by asserting the correct behavior and panicking because the current code is broken
2. **Second commit: Fix the bug** - Implement the fix
3. **Third commit: Remove `#[should_panic]`** - The test now passes without panicking

Example:
```rust
// Commit 1: Test that documents the bug
#[test]
#[should_panic(expected = "Hooks ran in wrong order")]
fn test_hook_ordering() {
    // This test asserts correct behavior
    // It panics because the bug exists
}

// Commit 2: Fix the bug (no test changes)

// Commit 3: Remove should_panic
#[test]
fn test_hook_ordering() {
    // Same test, now passes
}
```

This approach:
- Documents the bug exists (test demonstrates it)
- Proves the fix works (test passes after fix)
- CI passes on every commit
- Creates a clear commit history showing bug -> fix -> verification

### What to avoid

- Commits that break CI (even temporarily)
- "WIP" or "fixup" commits in the final history
- Large commits that do multiple unrelated things

### While working on the branch

While actively developing on the branch, feel free to:
- Make messy commits
- Squash things together
- Experiment and revert
- Push work-in-progress
- **Break CI** - it's fine, you'll fix it
- **Use `--no-verify`** to skip the pre-commit hook and let CI validate instead. The pre-commit hook is slow (fmt, schema regen, clippy, tests, udeps). Committing with `--no-verify` and pushing keeps you working while CI runs in the background. Fix forward if CI fails.

The branch is your workspace for experimentation. Push wild changes, figure out what works, debug why CI is breaking. Don't worry about commit cleanliness until you're ready to merge.

### Before merging to master

Do a final pass to make sure the branch is ready:

1. Review the full diff from master
2. Clean up commits if needed (rebase -i to reorganize)
3. Each commit on the branch should pass CI independently
4. Push the cleaned-up branch and verify CI passes
5. Only then squash merge to master

### Merging to master

**Every commit on master must pass CI. Squash merge feature branches.**

The workflow:
1. Every commit on your branch passes CI independently
2. The branch as a whole passes CI
3. Squash merge to master → one commit per branch

```bash
git checkout master
git merge --squash feature-branch
git commit -m "Description of the change"
git push
```

Atomic commits live on the branch for development and debugging. Master gets a clean, linear history where each commit is a complete, self-contained change that passes CI.

## Extract sub-refactors

**This happens during design, not after approval.** While writing the architecture document, actively compare the target code against the current code. Whenever the target and current code share the same logic with only structural differences (decoupled from I/O, different ownership, deferred side effects), that structural change is a sub-refactor that can land independently.

Each sub-refactor gets its own markdown file in `refactors/pending/`. It has its own motivation, its own before/after, and can be implemented and merged without the parent refactor existing. The parent document references it by filename.

The goal: shrink the parent refactor's diff to only the irreducible core — the thing that can't be done incrementally. Every line of code that can be changed in advance, should be.

### How to find sub-refactors

Compare each proposed component against its current equivalent:

1. If the proposed code is the same logic with I/O removed, the I/O extraction is a sub-refactor.
2. If the proposed code moves state from one struct to another, the extraction is a sub-refactor.
3. If the proposed code replaces an inline side effect with a deferred mechanism (e.g., inline `schedule_finally()` becomes deferred `removed_parents`), that deferral is a sub-refactor.
4. If the proposed code removes fields from a struct and looks them up instead, that field removal is a sub-refactor.
5. If the proposed code replaces a state machine variant with a counter, that simplification is a sub-refactor.

6. If the proposed code renames a type, field, or function, that rename is a sub-refactor. Renames are mechanical and land trivially.
7. If the proposed code adds derives to existing types (e.g., `Serialize`, `Clone`), those derives can land independently.
8. If the proposed code deletes unused fields, variants, or functions, each deletion is a sub-refactor.

Each of these can land on master independently and pass CI. After they all land, the parent refactor's diff is smaller, focused, and less risky.

### Why separate files

Large refactor documents become unmanageable. When a document covers both "extract RunState from TaskRunner" and "introduce the Applier trait", the scope is too wide to review or implement cleanly. Sub-refactors in separate files can be approved, implemented, and completed independently. The parent document stays focused on the architectural change that requires everything to happen together.

### Examples

A refactor that introduces an event loop with appliers might produce:
- `EXTRACT_RUN_STATE.md` — move task tracking into its own struct
- `REMOVE_INFLIGHT_VARIANT.md` — replace InFlight state with a counter
- `REMOVE_CONFIG_FROM_TASK_ENTRY.md` — drop cached config fields
- `DEFER_PARENT_REMOVAL.md` — accumulate removed parents instead of inline finally scheduling
- `APPLY_PATTERN.md` — the parent refactor, now a much smaller diff

## Completing refactors

**IMPORTANT: When a refactor is fully implemented and merged to master, move it from `refactors/pending/` to `refactors/past/` immediately.** Do this automatically — do not wait for the user to ask. Do not leave completed refactors in `pending/`.

The target directory is `refactors/past/` — not `done/`, not `completed/`, not anywhere else. `past/` already exists.

This keeps the pending folder focused on active work and preserves completed designs for reference.
