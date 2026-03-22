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

**Work on feature branches with atomic commits. CI must pass on every commit.**

When implementing a refactor:

1. **Create a feature branch** for the refactor (e.g., `refactor/crossbeam-channels`)
2. **Make atomic commits where CI passes on every commit** - each commit should be a complete, working state
3. **Push branches and verify CI is green** before moving to the next commit
4. **When the branch is fully ready, squash merge to master** - one clean commit per branch

### Stacked branches for CI verification

**Push branches to verify CI passes at each step.** You don't need to create PRs for every commit - just push the branch and check CI. This is especially important for multi-commit refactors.

Example workflow for a 3-commit refactor:
1. Create `refactor/foo-step-1` with the first commit, push branch, verify CI
2. Create `refactor/foo-step-2` on top with the second commit, push branch, verify CI
3. Create `refactor/foo` (main branch) on top with the final commit, push branch, verify CI
4. When all green, squash merge the main branch to master

This ensures:
- Each commit passes CI independently
- You can identify exactly which commit breaks CI during development
- Master has a clean, linear history of squashed commits

When rebasing the stack, rebase from bottom to top:
```bash
git checkout refactor/foo-step-1 && git rebase master
git checkout refactor/foo-step-2 && git rebase refactor/foo-step-1
git checkout refactor/foo && git rebase refactor/foo-step-2
```

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

## Extract independent work (after approval)

**Critical**: After receiving approval, actively identify changes that are independent of the main refactor. These should be:

1. **Extracted** into their own small changes
2. **Implemented first** before the main refactor
3. **Marked as done** in the plan

Examples of independent work:
- Format changes that don't affect behavior
- Removing dead code
- Adding new enum variants (before using them)
- Refactoring internal structure (typestate patterns, etc.)

The goal: by the time you start the "real" refactor, as much preliminary work as possible is already done. The remaining changes are the irreducible core that must happen together.

## Why this matters

- Smaller PRs are easier to review
- Independent changes can be tested in isolation
- If the main refactor is abandoned, the preliminary work still has value
- Reduces risk by making each change smaller

## Completing refactors

When a refactor is complete, move it from `refactors/pending/` to `refactors/past/`. This keeps the pending folder focused on active work and preserves completed designs for reference.
