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

## Task

First, `cd` into `local_dir`:

```bash
cd <local_dir>
```

A cherry-pick onto `parent_branch` failed with merge conflicts. The cherry-pick is still in progress (not aborted).

### 1. Read the original commit message

The commit message is preserved at `.git/MERGE_MSG`. Read it — your goal is to preserve this commit's intent through the conflict resolution.

### 2. Identify conflicts

Run `git status` to see which files have conflicts. Examine each conflicted file.

### 3. Resolve conflicts

For each conflicted file:
- Read the file and understand the conflict markers (`<<<<<<<`, `=======`, `>>>>>>>`)
- Resolve the conflict by keeping the correct version. In most cases:
  - **The parent's changes** (from `parent_branch`) represent validated, finalized work
  - **The current branch's changes** represent the commit being cherry-picked
  - Merge them correctly, preserving both sets of changes where they don't overlap
- Remove all conflict markers
- The resolution must preserve the intent of the original commit message you read in step 1

### 4. Complete the cherry-pick

After resolving all conflicts:

```bash
git add -A
git cherry-pick --continue
```

### 5. Verify

Run `git log --oneline -3` to confirm the branch looks correct.

## Output

Emit CheckDiff to verify the branch has meaningful changes:

```json
[{"kind": "CheckDiff", "value": {"branch_name": "...", "parent_branch": "...", "local_dir": "..."}}]
```

Do **not** include `conflict_output` in the output value.
