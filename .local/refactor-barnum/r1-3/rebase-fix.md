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

## Task

First, `cd` into `local_dir`:

```bash
cd <local_dir>
```

A cherry-pick onto `parent_branch` failed with merge conflicts. The cherry-pick is still in progress (not aborted).

### 1. Understand the original change

Read the commit message from `.git/MERGE_MSG`. Then view the **exact diff** of the original commit:

```bash
git -c remote.origin.promisor=false show $(cat .git/CHERRY_PICK_HEAD) -- ':(exclude)*.snap'
```

This shows you precisely what the commit changed. This is the change you need to apply to the parent's updated code.

### 2. Check if the change is already on the parent

For each file the commit modifies, check whether the parent branch's current code already has the specific change applied. For example:
- If the diff adds a `useSlideWidth` function — does that function already exist on the parent?
- If the diff changes `||` to `??` on a specific line — does the parent already have `??` there?

**Compare the diff from step 1 against the parent's current code. Do not just look at whether the parent's code seems "newer" or "refactored" — different refactors may have changed the same area without applying THIS specific change.**

### 3. Resolve conflicts

Run `git status` to see which files have conflicts. For each conflicted file:

1. Read the file and find the conflict markers (`<<<<<<<`, `=======`, `>>>>>>>`)
2. The section between `<<<<<<<` and `=======` is the **parent branch's version**
3. The section between `=======` and `>>>>>>>` is the **cherry-picked commit's version**

**If the change is NOT already on the parent:** Apply the original commit's change (from step 1) to the parent's updated code. Start with the parent's version, then apply the specific modifications from the diff. Do NOT simply accept the parent version — that drops the change.

**If the change IS verifiably already on the parent:** Keep the parent's version. The cherry-pick will be empty after resolution.

### 4. Complete the cherry-pick

After resolving all conflicts:

```bash
git add -A
git -c core.hooksPath=/dev/null cherry-pick --continue
```

**You MUST use `-c core.hooksPath=/dev/null`** on the cherry-pick --continue. Without it, a pre-commit hook will block the commit.

If `cherry-pick --continue` says the cherry-pick is now empty, use `git cherry-pick --skip`.

### 5. Verify

Run `git log --oneline -3` to confirm the branch looks correct.

## Output

Emit CheckDiff to verify the branch has meaningful changes:

```json
[{"kind": "CheckDiff", "value": {"branch_name": "...", "parent_branch": "...", "local_dir": "..."}}]
```

Do **not** include `conflict_output` in the output value.
