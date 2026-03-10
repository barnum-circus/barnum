# Publishing a New Version

## Files to Update

Bump the version in all of these:

1. **`Cargo.toml`** — `[workspace.package] version`
2. **`crates/string_id/Cargo.toml`** — has its own `version` (not workspace)
3. **`libs/gsd/package.json`** — `@gsd-now/gsd` npm package
4. **`libs/agent_pool/package.json`** — `@gsd-now/agent-pool` npm package

**Do NOT bump `docs-website/package.json`** — it stays at `0.0.0` (not published).

## Internal Crate Dependencies

Internal crate dependencies use `path = "..."` only, no `version` field. There's nothing to update for these.

## Verify

```bash
cargo check
```

This ensures all Cargo.toml versions are consistent.

## Commit, Tag, Push

```bash
git add -A
git commit -m "v0.X.0"
git tag v0.X.0
git push origin master --tags
```

## npm Publish

CI handles npm publishing when a tag is pushed (if configured), or publish manually:

```bash
cd libs/gsd && npm publish --access public
cd libs/agent_pool && npm publish --access public
```
