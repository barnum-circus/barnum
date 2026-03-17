# RebaseFix

Resolve cherry-pick conflicts and complete the cherry-pick.

## Input

- `branch_name`: The branch being updated.
- `parent_branch`: The branch being cherry-picked onto.
- `local_dir`: Path to the webapp root directory. **Run all commands from this directory.**
- `conflict_output`: Output from the failed `git cherry-pick`, showing which files conflict.

## Environment

You are running inside a sandbox **without network access**. Before running any git commands, set:

```bash
export GIT_LFS_SKIP_SMUDGE=1
export GIT_TERMINAL_PROMPT=0
```

**Disable git hooks** on all git commands that create commits:

```bash
git -c core.hooksPath=/dev/null cherry-pick --continue
git -c core.hooksPath=/dev/null commit ...
```

## Critical: Why Conflicts Happen

Conflicts happen for one of two reasons:

1. **Surrounding code changed** — the cherry-picked change is NOT on the parent yet, but context shifted. You must apply the change to the updated code.
2. **The change is already on the parent** — the parent already has the same logical change (merged via a different PR). In this case, keeping the parent's version is correct and the cherry-pick will be empty.

Read the commit message, examine the conflict, and determine which case applies. Check whether the parent's code already reflects the commit's intended change before deciding.

## Task

First, `cd` into `local_dir`:

```bash
cd <local_dir>
```

A cherry-pick onto `parent_branch` failed with merge conflicts. The cherry-pick is still in progress (not aborted).

### 1. Read the original commit message

The commit message is preserved at `.git/MERGE_MSG`. Read it — this describes the change that MUST be preserved through conflict resolution.

### 2. Understand the intended change

Before looking at conflicts, understand what this commit is trying to do. The commit message describes a specific refactor or fix. Your job is to apply that same logical change to the updated code on `parent_branch`.

### 3. Identify and resolve conflicts

Run `git status` to see which files have conflicts. For each conflicted file:

1. Read the file and find the conflict markers (`<<<<<<<`, `=======`, `>>>>>>>`)
2. The section between `<<<<<<<` and `=======` is the **parent branch's version** (the updated surrounding code)
3. The section between `=======` and `>>>>>>>` is the **cherry-picked commit's version** (the intended change applied to old surrounding code)
4. **Resolve by applying the commit's intended change to the parent's updated code.** This means:
   - Start with the parent branch's version (the updated surrounding code)
   - Apply the specific modification described in the commit message to that updated code
   - The result should reflect BOTH the parent's updated context AND the commit's intended change

**Do NOT blindly accept the parent version.** If the change is NOT already on the parent, accepting the parent version drops it. But if the parent already has the change (merged via another PR), then keeping the parent version is correct.

### 4. Complete the cherry-pick

After resolving all conflicts:

```bash
git add -A
git -c core.hooksPath=/dev/null cherry-pick --continue
```

**You MUST use `-c core.hooksPath=/dev/null`** on the cherry-pick --continue. Without it, a pre-commit hook will block the commit.

If `cherry-pick --continue` says the cherry-pick is now empty (because the change is already on the parent), use `git cherry-pick --skip` to proceed. This is expected for branches whose changes were already merged upstream.

### 5. Verify

Run `git log --oneline -3` to confirm the branch looks correct.

## Output

Emit CheckDiff to verify the branch has meaningful changes:

```json
[{"kind": "CheckDiff", "value": {"branch_name": "...", "parent_branch": "...", "local_dir": "..."}}]
```

Do **not** include `conflict_output` in the output value.
