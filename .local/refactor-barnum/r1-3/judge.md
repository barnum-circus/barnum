# Judge

Review the change on this branch for correctness, completeness, and safety.

## Input

- `branch_name`: The branch to review.
- `parent_branch`: The parent branch.
- `local_dir`: Path to the webapp root directory. **Run all commands from this directory.**

## Environment

You are running inside a sandbox **without network access**. Before running any git commands, set:

```bash
export GIT_LFS_SKIP_SMUDGE=1
export GIT_TERMINAL_PROMPT=0
```

For all git commands, pass `-c remote.origin.promisor=false` to prevent network fetches:

```bash
git -c remote.origin.promisor=false diff ...
git -c remote.origin.promisor=false log ...
```

Limit diffs to the `webapp/` directory to avoid partial clone issues:

```bash
git -c remote.origin.promisor=false diff <parent_branch>..HEAD -- webapp/
```

## Task

First, `cd` into `local_dir`:

```bash
cd <local_dir>
```

You are on branch `branch_name`. Lint, relay, and tsc all pass. Now review the actual change.

Run `git -c remote.origin.promisor=false diff <parent_branch>..HEAD -- webapp/` to see the full diff for this branch. **Always use the `parent_branch` value from the input — never substitute commit SHAs.**

### 1. Classification: safe refactor vs. bug fix

Determine whether this change is:

- **A provably safe refactor** -- behavior is identical before and after. Examples: extracting constants, renaming internal variables, moving code without changing logic.
- **A bug fix** -- the change actually corrects incorrect behavior. Examples: fixing a null check that was wrong, correcting a comparison operator.

If it's a bug fix, amend the commit message to note this:
```bash
git commit --amend -m "fix: <description of what was actually broken>"
```

### 2. Empty string assumptions

Check that the change does not rely on the assumption that a string will never be empty. For example:

```ts
// BAD: assumes `name` is never ""
if (name) { ... }

// GOOD: explicitly checks for the condition you care about
if (name !== undefined && name !== null) { ... }
```

If the refactor introduces or preserves a truthiness check on a string, verify that empty string behavior is correct.

### 3. "Push ifs up" correctness

If this refactor pushes an `if` check up (e.g., converting an early return into a caller-side check), verify:

- **The parameter is made non-optional.** If the original code had `if (!x) return;` and we removed that guard, the parameter `x` must now be required (not optional).
- **All call sites pass the value.** If the parameter was previously optional and is now required, every caller must be updated.

### 4. Related refactors in the same file

Look at the rest of the file. If this commit does something (e.g., extracts a constant, removes dead code), check whether there are other instances of the same pattern in the same file that should also be addressed.

### 5. External API changes -- caller verification

If the change affects a publicly exported function, component, or type, you MUST verify all callers are correct:

1. **Find all callers**: search for imports of the modified symbol.
2. **Verify each call site** is compatible with the change.

### 6. Cascading changes

Modifications to callers may trigger further necessary changes. Follow the chain until it terminates. Each cascading change should be included in this same commit (amend).

### 7. If you made any changes

Stage, amend, and emit Validate:

```bash
git add -A
git commit --amend --no-edit
```

## Output

If the change is correct and complete with no modifications needed:

```json
[]
```

If you made changes and need to re-validate:

```json
[{"kind": "Validate", "value": {"branch_name": "...", "parent_branch": "...", "local_dir": "..."}}]
```
