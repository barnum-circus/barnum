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

### 1. Read the original commit message

The commit message is preserved at `.git/MERGE_MSG`. Read it — this describes the change that MUST be preserved through conflict resolution.

### 2. Understand the intended change

Before looking at conflicts, understand what this commit is trying to do. The commit message describes a specific refactor or fix — for example, "Extract useSlideWidth hook" or "Remove redundant optional chaining."

### 3. Check if the change is already on the parent

Search the parent branch's code for the **specific artifact** described in the commit message. For example:
- If the commit says "Extract useSlideWidth hook" — does a `useSlideWidth` function already exist?
- If the commit says "Remove redundant optional chaining on `pin`" — is the optional chaining already removed?

**Look for the exact thing the commit describes, not whether the surrounding code looks "updated" or "newer."** The parent may have other refactors to the same file that are unrelated to this commit's change.

### 4. Resolve conflicts

Run `git status` to see which files have conflicts. For each conflicted file:

1. Read the file and find the conflict markers (`<<<<<<<`, `=======`, `>>>>>>>`)
2. The section between `<<<<<<<` and `=======` is the **parent branch's version**
3. The section between `=======` and `>>>>>>>` is the **cherry-picked commit's version**

**If the change is NOT already on the parent (step 3):** Resolve by applying the commit's intended change to the parent's updated code. Start with the parent's version, then apply the specific modification. Do NOT simply accept the parent version — that drops the change.

**If the change IS already on the parent (step 3):** Keep the parent's version. The cherry-pick will be empty after resolution.

### 5. Complete the cherry-pick

After resolving all conflicts:

```bash
git add -A
git -c core.hooksPath=/dev/null cherry-pick --continue
```

**You MUST use `-c core.hooksPath=/dev/null`** on the cherry-pick --continue. Without it, a pre-commit hook will block the commit.

If `cherry-pick --continue` says the cherry-pick is now empty, use `git cherry-pick --skip`.

### 6. Verify

Run `git log --oneline -3` to confirm the branch looks correct.

## Output

Emit CheckDiff to verify the branch has meaningful changes:

```json
[{"kind": "CheckDiff", "value": {"branch_name": "...", "parent_branch": "...", "local_dir": "..."}}]
```

Do **not** include `conflict_output` in the output value.
