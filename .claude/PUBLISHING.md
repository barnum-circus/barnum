# Publishing a New Version

## Files to Update

Bump the version in all of these:

1. **`Cargo.toml`** — `[workspace.package] version`
2. **`crates/intern/Cargo.toml`** — has its own `version` (not workspace, forked crate — only bump if changed)
3. **`libs/barnum/package.json`** — `@barnum/barnum` npm package

**Do NOT bump `docs-website/package.json`** — it stays at `0.0.0` (not published).

## Internal Crate Dependencies

Internal crate dependencies use `path = "..."` only, no `version` field. There's nothing to update for these.

## Update Lock Files

```bash
cargo check          # regenerates Cargo.lock
pnpm install --lockfile-only  # regenerates pnpm-lock.yaml (if version appears in it)
```

## Verify

```bash
cargo check
cargo test --locked --workspace -- --test-threads=1
```

`--locked` ensures `Cargo.lock` is up to date (CI uses this flag).

## Commit, Tag, Push

```bash
git add -A
git commit -m "v0.X.0"
git tag v0.X.0
git push origin master --tags
```

## Docs Website Version (minor/major releases only)

Cut a new docs version snapshot. Skip this for patch releases.

```bash
cd docs-website && pnpm exec docusaurus docs:version 0.X
```

This copies `docs/` into `versioned_docs/version-0.X/` and updates `versions.json`.

## npm Publish

CI handles npm publishing when a tag is pushed (if configured), or publish manually:

```bash
cd libs/barnum && npm publish --access public
```
