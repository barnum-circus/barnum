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

**Conflicts do NOT mean the change is already applied.** The cherry-picked commit makes a specific, targeted change (described in the commit message). The conflict happens because the **surrounding code** on `parent_branch` has changed since the commit was originally authored. The cherry-picked change itself is almost certainly NOT on the parent branch.

**You MUST apply the branch's intended change to the updated code.** If after your resolution the diff between `parent_branch` and `branch_name` is empty, you resolved it wrong — you accidentally dropped the change instead of applying it.

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

**Do NOT resolve conflicts by simply accepting the parent version.** That drops the change entirely. The whole point of this step is to preserve the change.

### 4. Complete the cherry-pick

After resolving all conflicts:

```bash
git add -A
git -c core.hooksPath=/dev/null cherry-pick --continue
```

**You MUST use `-c core.hooksPath=/dev/null`** on the cherry-pick --continue. Without it, a pre-commit hook will block the commit.

### 5. Verify the change was preserved

Run:

```bash
git -c remote.origin.promisor=false diff <parent_branch>..HEAD -- :/webapp/
```

**This diff MUST NOT be empty.** If it is empty, you dropped the change during conflict resolution. Go back and fix it — the commit's intended change was lost.

The diff should show the logical change described in the commit message, applied to the current state of the code.

### 6. Verify commit history

Run `git log --oneline -3` to confirm the branch looks correct.

## Output

Emit CheckDiff to verify the branch has meaningful changes:

```json
[{"kind": "CheckDiff", "value": {"branch_name": "...", "parent_branch": "...", "local_dir": "..."}}]
```

Do **not** include `conflict_output` in the output value.
