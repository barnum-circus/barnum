# Refactor Process

## CRITICAL: Never Start Without Explicit Approval

**DO NOT begin any implementation work without explicit user approval.** This includes:
- Writing code
- Creating branches
- Running tests on proposed changes
- Committing anything
- "Investigating" by making changes

When the user describes a task or asks you to design something:
1. Write the plan/design document
2. Present it to the user
3. **STOP AND WAIT** for explicit approval ("go ahead", "implement it", "approved")
4. Only then begin implementation

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
3. **Push and verify CI is green** before moving to the next commit
4. **When the branch is fully ready, merge to master without squashing** - preserve the atomic commit history

### Stacked branches for CI verification

**Use one branch per commit to verify CI passes at each step.** This is especially important for multi-commit refactors.

Example workflow for a 3-commit refactor:
1. Create `refactor/foo-step-1` with the first commit, push, verify CI
2. Create `refactor/foo-step-2` on top with the second commit, push, verify CI
3. Create `refactor/foo` (main branch) on top with the final commit, push, verify CI
4. When all green, merge the main branch to master

This ensures:
- Each commit passes CI independently
- You can identify exactly which commit breaks CI
- Bisecting is meaningful at every point in the stack

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
- Squashing - we want the atomic commits preserved on master
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

Do a final pass to restructure the branch into clean atomic commits:

1. Review the full diff from master
2. Use `git rebase -i` to reorganize commits
3. Each commit should be one logical change that passes CI
4. Push the cleaned-up branch and verify CI passes on all commits
5. Only then merge to master

### Merging to master

When the branch is complete and CI is green on all commits:

```bash
git checkout master
git merge --no-ff feature-branch  # or fast-forward if linear
git push
```

Do NOT squash. The atomic commits are the value - they make master bisectable and each change understandable in isolation.

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
