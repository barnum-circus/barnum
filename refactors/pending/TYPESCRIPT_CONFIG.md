# TypeScript Config Files

**Status:** Pending — types exist (via `ZOD_SCHEMA.md`), runtime `.ts` config loading does not.

## Motivation

The Zod schema work (`barnum-config-schema.zod.ts`) provides TypeScript types and a `defineConfig` helper. Users can already write:

```typescript
import { defineConfig } from "@barnum/barnum";

export default defineConfig({
  entrypoint: "Analyze",
  steps: [
    { name: "Analyze", action: { kind: "Pool", instructions: { kind: "Inline", value: "..." } }, next: [] },
  ],
});
```

But `barnum run --config workflow.ts` doesn't work — the CLI only accepts JSON/JSONC files. To close the loop, the CLI needs to detect `.ts`/`.mts` extensions, evaluate the file, and use the default export as the config object.

This unlocks programmatic config generation: loops, shared step definitions, conditional steps, computed instructions — all with full type checking.

## Current State

### Config loading

`crates/barnum_cli/src/main.rs:419-460` — `parse_config()` handles two cases:
1. If the input is a file path that exists, read it and parse as JSONC via `json5::from_str`
2. Otherwise, treat it as inline JSON/JSONC

No TypeScript evaluation exists.

### Types and defineConfig

Already shipped in `libs/barnum/barnum-config-schema.zod.ts`:
- `configFileSchema` — Zod schema for the full config
- `export type ConfigFile = z.infer<typeof configFileSchema>`
- `export function defineConfig(config) → ConfigFile` — identity function with type inference

The npm package (`@barnum/barnum`) already has `"types": "barnum-config-schema.zod.ts"` and `zod` as an optional peer dependency.

## Proposed Changes

### Task 1: Evaluate `.ts` config files in the CLI

**Goal:** When `--config` points to a `.ts` or `.mts` file, evaluate it with `npx tsx`, capture the default export as JSON, and parse that.

**File:** `crates/barnum_cli/src/main.rs`

The approach: shell out to `npx tsx` with a wrapper script that imports the config file and writes `JSON.stringify(module.default)` to stdout. Parse that stdout as the config.

```rust
// In parse_config, before the json5 parse:
fn parse_config(input: &str) -> io::Result<(ConfigFile, PathBuf)> {
    let path = PathBuf::from(input);
    if path.exists() {
        let canonical = path.canonicalize().map_err(|e| { ... })?;
        let dir = canonical.parent().unwrap_or(Path::new(".")).to_path_buf();

        let content = match path.extension().and_then(|e| e.to_str()) {
            Some("ts" | "mts") => evaluate_ts_config(&canonical)?,
            _ => std::fs::read_to_string(&path).map_err(|e| { ... })?,
        };

        let cfg: ConfigFile = json5::from_str(&content).map_err(|e| { ... })?;
        Ok((cfg, dir))
    } else {
        // inline JSON/JSONC (unchanged)
        ...
    }
}
```

The `evaluate_ts_config` function:

```rust
fn evaluate_ts_config(path: &Path) -> io::Result<String> {
    let output = std::process::Command::new("npx")
        .arg("tsx")
        .arg("--eval")
        .arg(format!(
            "import c from {path}; process.stdout.write(JSON.stringify(c))",
            path = serde_json::to_string(&path.display().to_string()).unwrap(),
        ))
        .output()
        .map_err(|e| io::Error::new(e.kind(), format!(
            "[E057] failed to run npx tsx for {}: {e}. Is tsx installed? (npm install -g tsx)",
            path.display()
        )))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("[E058] TypeScript config evaluation failed for {}: {stderr}", path.display()),
        ));
    }

    String::from_utf8(output.stdout).map_err(|e| io::Error::new(
        io::ErrorKind::InvalidData,
        format!("[E059] TypeScript config output was not valid UTF-8: {e}"),
    ))
}
```

**Complication: `npx tsx --eval` with ESM imports.** The `--eval` approach may need `--input-type=module` or a different invocation. Need to verify tsx's CLI for evaluating inline code that uses ESM `import`. Alternative: write a temp `.mjs` wrapper file that imports the user's config and prints JSON. This avoids eval quirks.

**Complication: `tsx` availability.** If `tsx` isn't installed, the error should be clear. The error message above points the user to `npm install -g tsx`. We could also try `ts-node` as a fallback, but tsx is simpler (no tsconfig required) and is the standard choice now.

**Complication: relative imports in the config file.** The user's `.ts` config may import other local files. The working directory for the `npx tsx` process should be the config file's parent directory so relative imports resolve correctly.

### Task 2: Add CLI tests for `.ts` config files

**Goal:** Integration tests that verify `.ts` configs work end-to-end.

**File:** `crates/barnum_cli/tests/config_subcommands.rs`

Add tests:
1. `validate_ts_config` — write a `.ts` config to a temp dir, run `barnum config validate --config config.ts`, verify success
2. `run_ts_config` — write a `.ts` config, run it with a command action, verify tasks execute
3. `ts_config_error` — write an invalid `.ts` config (type error or syntax error), verify the error message is clear

These tests will need `tsx` available in CI. Add it as a dev dependency or install it in the CI workflow.

### Task 3: Update docs

**Files:** `docs-website/docs/reference/config-schema.md`, `docs-website/docs/quickstart.md`

Add a section showing TypeScript config usage:

```markdown
## TypeScript Configs

Write configs in TypeScript for type checking and programmatic generation:

\`\`\`typescript
// workflow.ts
import { defineConfig } from "@barnum/barnum";

const analyzeSteps = ["src", "lib", "tests"].map(dir => ({
  name: `Analyze-${dir}`,
  action: { kind: "Pool" as const, instructions: { kind: "Inline" as const, value: `Analyze ${dir}` } },
  next: [] as string[],
}));

export default defineConfig({
  steps: analyzeSteps,
});
\`\`\`

\`\`\`bash
barnum run --config workflow.ts
\`\`\`

Requires `tsx` (`npm install -g tsx` or as a project dev dependency).
```

## Open Questions

1. **`tsx` vs `ts-node` vs `esbuild` + eval.** `tsx` is the simplest (zero config, handles ESM natively). `ts-node` requires tsconfig. Raw `esbuild` bundling + `node --eval` is faster but more complex. Leaning `tsx` for simplicity.

2. **Should `barnum config validate` also accept `.ts` files?** Yes — `validate`, `docs`, and `graph` all call `parse_config`, so they'd get `.ts` support for free once `parse_config` handles it.

3. **Should we bundle tsx or require it as a user dependency?** Requiring it keeps our binary lean. The error message when tsx is missing should be actionable.
