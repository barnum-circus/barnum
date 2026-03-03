# Refactor Process

Big refactors follow a two-phase process:

## Phase 1: Architecture document

Create a markdown file in `refactors/pending/` describing:
- Motivation and goals
- Current state (with line numbers and code snippets)
- Proposed changes at the architectural level
- Open questions and design decisions

This document captures the *shape* of the refactor without getting into implementation details.

## STOP: Wait for approval

**After writing the architecture document, STOP and wait for explicit user approval before implementing ANYTHING.** Do not:
- Start implementing tasks
- Make "small independent changes"
- Commit code changes
- Push anything

The document exists for the user to review. They may have feedback, want changes to the approach, or decide not to proceed at all. **Only begin implementation after the user explicitly says to proceed.**

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

**Work on feature branches, merge clean commits to master.**

When implementing a refactor:

1. **Create a feature branch** for the refactor (e.g., `refactor/crossbeam-channels`)
2. **Do all exploratory work on the branch** - experiments, debugging, test fixes, false starts
3. **Only merge to master when ready** - clean, tested, atomic commits

What stays on the branch until ready:
- Debug logging and temporary diagnostics
- Failed approaches and reverts
- Work-in-progress commits
- Test fixes that are still being validated

What gets merged to master:
- Clean, atomic commits that pass all tests
- Each commit should be independently deployable
- Commit messages should explain the "why", not the exploration process

This keeps master clean and bisectable. The branch preserves the full history of how you got there, but master only sees the polished result.

**Cherry-pick individual commits** from the feature branch to master as they become ready, rather than waiting for the entire refactor to complete. This way independent improvements land incrementally.

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
