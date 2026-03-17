# CompleteRefactor

Propagate public API changes from this branch's commit to all downstream call sites.

## Input

- `branch_name`: The branch to process.
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

You are on branch `branch_name`. The rebase is complete. Before running validation (tsc, lint, relay), you need to propagate any public API changes made in this commit to all callers.

### 1. Read the diff

```bash
git -c remote.origin.promisor=false diff <parent_branch>..HEAD -- webapp/
```

**Always use the `parent_branch` value from the input -- never substitute commit SHAs.**

Identify any changes to the **public API** of exported functions, components, types, or hooks. Specifically look for:

- **Removed props/parameters**: A function or component parameter that no longer exists.
- **Changed props/parameters**: A parameter whose type or name changed.
- **Narrowed return types**: A function that previously returned `T | null` now returns `T`, or a union type that lost a variant.
- **Renamed exports**: A symbol that was renamed.

If the diff contains no public API changes (e.g., purely internal logic changes), skip to the Output section -- there's nothing to propagate.

### 2. Find all call sites

For each modified export, find every file that imports or uses it:

```bash
grep -rn "import.*symbolName" --include="*.ts" --include="*.tsx" .
```

Also search for re-exports, dynamic references, and any other usage patterns.

### 3. Update call sites

For each call site, make the minimum necessary changes:

- **Removed props**: Remove the prop from the call site. If the caller was computing a value solely to pass as that prop, remove the computation too (it's now dead code).
- **Changed props**: Update the call site to pass the prop in the new way.
- **Narrowed return types**: If the caller was handling a case that can no longer occur (e.g., a null check on a value that's now non-nullable), remove the dead branch. Keep the logic that handles the now-guaranteed case.
- **Renamed exports**: Update the import and all references.

### 4. Follow the chain

Your changes to call sites may themselves create further cascading changes. For example:

- Removing a prop from component A's call in component B -> the variable in B that held the value is now unused -> remove it -> if B received it as a prop, remove it from B's props type -> find B's callers -> repeat.
- Narrowing a return type used in a caller -> the caller's own return type may narrow -> the caller's callers may have dead code -> repeat.

**Follow this chain as many layers deep as necessary.** Each layer may trigger the next. Stop only when there are no more downstream changes to make.

### 5. Stage and amend

After all changes:

```bash
git add -A
git commit --amend --no-edit
```

## Output

Emit Validate to run lint/tsc/relay:

```json
[{"kind": "Validate", "value": {"branch_name": "...", "parent_branch": "...", "local_dir": "..."}}]
```
