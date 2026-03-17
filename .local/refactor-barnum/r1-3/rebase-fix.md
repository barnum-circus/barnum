# RebaseFix

Resolve rebase conflicts and complete the rebase.

## Input

- `branch_name`: The branch being rebased.
- `parent_branch`: The branch being rebased onto.
- `local_dir`: Path to the webapp root directory. **Run all commands from this directory.**
- `conflict_output`: Output from the failed `git rebase`, showing which files conflict.

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

The rebase of `branch_name` onto `parent_branch` failed with merge conflicts. The rebase is still in progress (not aborted).

### 1. Read the original commit message

The commit message is preserved during rebase at `.git/rebase-merge/message`. Read it — your goal is to preserve this commit's intent through the conflict resolution.

### 2. Identify conflicts

Run `git status` to see which files have conflicts. Examine each conflicted file.

### 3. Resolve conflicts

For each conflicted file:
- Read the file and understand the conflict markers (`<<<<<<<`, `=======`, `>>>>>>>`)
- Resolve the conflict by keeping the correct version. In most cases:
  - **The parent's changes** (from the rebase target) represent validated, finalized work
  - **The current branch's changes** represent the commit being rebased
  - Merge them correctly, preserving both sets of changes where they don't overlap
- Remove all conflict markers
- The resolution must preserve the intent of the original commit message you read in step 1

### 4. Complete the rebase

After resolving all conflicts:

```bash
git add -A
git rebase --continue
```

If `git rebase --continue` presents additional conflicts (multi-commit rebase), repeat steps 2-3.

### 5. Verify

Run `git log --oneline -3` to confirm the branch looks correct after the rebase.

## Output

Emit CompleteRefactor to propagate API changes before validation:

```json
[{"kind": "CompleteRefactor", "value": {"branch_name": "...", "parent_branch": "...", "local_dir": "..."}}]
```

Do **not** include `conflict_output` in the output value.
