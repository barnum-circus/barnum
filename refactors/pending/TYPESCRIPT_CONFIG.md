# TypeScript Config API

**Status:** Phase 1 — Architecture document

## Motivation

Barnum configs are JSON/JSONC files. The JSON Schema provides editor validation, but users writing configs programmatically (generating steps in a loop, computing values, sharing step definitions across workflows) have no type safety.

This refactor generates TypeScript type definitions from the existing JSON Schema and exports them from the `@barnum/barnum` npm package, so users can write typed config files.

## Current State

### Config types

`crates/barnum_config/src/config.rs` defines `ConfigFile` as the top-level serde struct. Each field derives `schemars::JsonSchema` for automatic JSON Schema generation.

### JSON Schema generation

`crates/barnum_config/src/bin/build_barnum_schema.rs` calls `config_schema()` (which calls `schemars::schema_for!(ConfigFile)`), serializes the result to JSON, and writes it to `libs/barnum/barnum-config-schema.json`. This file is checked in and verified by CI.

### npm package

`libs/barnum/package.json` publishes `@barnum/barnum` (v0.2.4) with:
- `index.js` — resolves the platform-specific binary path
- `cli.js` — shebang wrapper that spawns the binary
- `barnum-config-schema.json` — JSON Schema
- `artifacts/` — pre-built binaries

The package has no TypeScript type definitions and no `.d.ts` files.

## Proposed Changes

### 1. Generate TypeScript types from the JSON Schema

The build pipeline becomes:

```
Rust types (schemars) --> barnum-config-schema.json --> types.d.ts
```

A Node script in `libs/barnum/` converts the JSON Schema to TypeScript definitions using `json-schema-to-typescript` (established npm package, 3k+ GitHub stars, handles JSON Schema draft-07). The script runs after `build_barnum_schema` regenerates the JSON Schema.

```bash
# In libs/barnum/
node build-types.js
```

The script reads `barnum-config-schema.json`, produces `types.d.ts`, and appends a `defineConfig` helper signature:

```typescript
// Generated from barnum-config-schema.json — do not edit manually.

export interface ConfigFile {
  entrypoint?: string;
  options?: Options;
  steps: StepFile[];
}

export interface Options {
  timeout?: number;
  max_retries?: number;
  max_concurrency?: number;
  retry_on_timeout?: boolean;
  retry_on_invalid_response?: boolean;
}

export interface StepFile {
  name: string;
  value_schema?: unknown;
  pre?: PreHook;
  action: ActionFile;
  post?: PostHook;
  next?: string[];
  finally?: FinallyHook;
  options?: StepOptions;
}

// ... (ActionFile, PreHook, PostHook, FinallyHook, StepOptions, etc.)

/**
 * Identity function that provides type inference for config objects.
 * Returns its argument unchanged.
 */
export declare function defineConfig(config: ConfigFile): ConfigFile;
```

The exact output depends on `json-schema-to-typescript`'s mapping of the schema. The above is illustrative. The generated file is checked in alongside the schema and verified by CI (same diff-check pattern).

**Build command sequence:**
```bash
cargo run -p barnum_config --bin build_barnum_schema   # Rust -> JSON Schema
cd libs/barnum && node build-types.js                   # JSON Schema -> types.d.ts
```

### 2. Export types from the npm package

New files in `libs/barnum/`:

**`types.d.ts`** — generated TypeScript definitions (all config types).

**`index.d.ts`** — re-exports from `types.d.ts`:
```typescript
export type {
  ConfigFile,
  StepFile,
  ActionFile,
  Options,
  StepOptions,
  PreHook,
  PostHook,
  FinallyHook,
} from './types';

export { defineConfig } from './types';
```

**`defineConfig.js`** — runtime identity function:
```javascript
'use strict';
module.exports.defineConfig = function defineConfig(config) {
  return config;
};
```

`defineConfig` provides type inference at the call site without requiring a separate type annotation:

```typescript
import { defineConfig } from "@barnum/barnum";

export default defineConfig({
  entrypoint: "Analyze",
  steps: [
    { name: "Analyze", action: { kind: "Pool", instructions: { inline: "..." } }, next: ["Done"] },
    { name: "Done", action: { kind: "Command", script: "echo '[]'" }, next: [] },
  ],
});
```

Users who prefer not to install the package can use a type-only import instead:

```typescript
import type { ConfigFile } from "@barnum/barnum";

const config: ConfigFile = {
  steps: [/* ... */],
};
export default config;
```

Type-only imports are erased by every TS runtime (tsx, bun, tsc), so the file executes without `@barnum/barnum` in `node_modules`. The types are only needed for editor completion and `tsc` type-checking.

**package.json changes:**
```json
{
  "types": "index.d.ts",
  "files": [
    "index.js",
    "index.d.ts",
    "types.d.ts",
    "defineConfig.js",
    "cli.js",
    "artifacts/**/*",
    "barnum-config-schema.json"
  ]
}
```

### 3. User workflow

Install types for editor support:
```bash
pnpm add -D @barnum/barnum
```

Write a typed config:
```typescript
// workflow.ts
import { defineConfig } from "@barnum/barnum";

export default defineConfig({
  entrypoint: "Analyze",
  options: { max_retries: 3, max_concurrency: 5 },
  steps: [
    {
      name: "Analyze",
      action: { kind: "Pool", instructions: { link: "analyze.md" } },
      next: ["Implement"],
    },
    {
      name: "Implement",
      action: { kind: "Pool", instructions: { link: "implement.md" } },
      next: [],
    },
  ],
});
```

The file exports a typed `ConfigFile` object. How barnum consumes it at runtime (`barnum run --ts`, runtime discovery, step builder helpers) is covered in `TYPESCRIPT_RUNTIME.md`.

## Open Questions

1. **Should `build-types.js` be a dev dependency or a checked-in script?** It only runs during the build. If it's a script, we vendor `json-schema-to-typescript` or shell out to `npx`. If it's a dev dependency, `libs/barnum/` needs a `package-lock.json` or similar.

2. **Should the generated types use `interface` or `type`?** `json-schema-to-typescript` defaults to interfaces. Union types (like `ActionFile` which is `Pool | Command`) may need adjustment. We should verify the output and hand-edit if the generated types are awkward.
