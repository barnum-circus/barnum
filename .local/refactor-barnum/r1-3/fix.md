# Fix

Fix lint, type-check, or relay compiler errors that the Validate step caught.

## Input

- `branch_name`: The branch being fixed.
- `parent_branch`: The parent branch.
- `local_dir`: Path to the webapp root directory. **Run all commands from this directory.**
- `errors`: The error output from the failed validation commands (may include output from `pnpm relay`, `pnpm changed:fix`, and/or `pnpm tsc`).

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

You are on branch `branch_name`. The Validate step ran `pnpm relay`, `pnpm changed:fix`, and `pnpm tsc`, and one or more of them failed. The `errors` field contains all the error output.

### 1. Read the errors

Parse the `errors` field to understand what failed and why. Common issues:
- **TypeScript errors**: type mismatches, missing imports, unused variables
- **ESLint errors**: code style issues that auto-fix couldn't resolve
- **Prettier errors**: formatting issues (usually auto-fixed, rare to fail)
- **Relay compiler errors**: invalid GraphQL fragments, missing fields, type mismatches

### 2. Fix the issues

Edit the files to resolve all errors. Make minimal, targeted changes -- don't refactor beyond what's needed to fix the errors.

**You may need to edit files unrelated to this branch's primary change.** For example, a type change on this branch can cause TypeScript errors in other files that import the changed type. Fix those too -- the goal is a clean build, not just fixing the files this branch originally touched.

### 3. Stage and amend

After fixing:

```bash
git add -A
git -c core.hooksPath=/dev/null commit --amend --no-edit
```

This amends your fixes into the existing commit on this branch.

## Output

Emit Validate to re-run the validation commands:

```json
[{"kind": "Validate", "value": {"branch_name": "...", "parent_branch": "...", "local_dir": "..."}}]
```

Do **not** include `errors` in the output value -- a fresh validation will produce new error output if issues remain.
