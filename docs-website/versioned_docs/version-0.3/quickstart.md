---
image: /img/og/quickstart.png
---

# Quickstart

## Install

```bash
mkdir my-workflow && cd my-workflow
pnpm init
pnpm add @barnum/barnum zod
```

## Write handlers

Handlers are async functions wrapped in `createHandler`. Start with simple stubs — no LLM needed yet.

```ts
// handlers/steps.ts
import { createHandler } from "@barnum/barnum";
import { z } from "zod";
import { readdirSync, readFileSync, writeFileSync } from "fs";

export const listFiles = createHandler({
  outputValidator: z.array(z.string()),
  handle: async () => {
    return readdirSync("src", { recursive: true })
      .filter((f): f is string => typeof f === "string" && f.endsWith(".tsx"))
      .map((f) => `src/${f}`);
  },
}, "listFiles");

export const migrateComponent = createHandler({
  inputValidator: z.string(),
  outputValidator: z.object({ file: z.string(), migrated: z.boolean() }),
  handle: async ({ value: file }) => {
    const content = readFileSync(file, "utf-8");
    if (!content.includes("class ") || !content.includes("extends React.Component")) {
      return { file, migrated: false };
    }
    // Stub: just log for now, we'll replace this with Claude later
    console.log(`TODO: migrate ${file}`);
    return { file, migrated: false };
  },
}, "migrateComponent");
```

Each handler runs in its own isolated subprocess. It only sees its own input — never the full workflow.

## Compose a workflow

```ts
// run.ts
import { runPipeline } from "@barnum/barnum";
import { listFiles, migrateComponent } from "./handlers/steps.js";

runPipeline(
  listFiles.forEach(migrateComponent),
);
```

`listFiles` returns an array of file paths. `forEach` fans out — each file flows through `migrateComponent` in parallel.

## Run it

```bash
pnpm exec tsx run.ts
```

This runs the full pipeline. Right now `migrateComponent` is a stub, so it just logs `TODO` messages. The structure is in place — handlers, validators, fan-out, parallel execution.

## Add Claude

Replace the stub with a real LLM call. Two options:

### Option A: Claude CLI

Spawn `claude` as a subprocess. No SDK dependency, works with any Claude Code installation.

```ts
// handlers/lib.ts
import { spawn } from "child_process";

export function callClaude(args: {
  prompt: string;
  allowedTools?: string[];
}): Promise<string> {
  const cliArgs = [
    "-p", args.prompt,
    "--output-format", "text",
  ];
  if (args.allowedTools?.length) {
    cliArgs.push("--allowedTools", ...args.allowedTools);
  }
  return new Promise((resolve, reject) => {
    const child = spawn("claude", cliArgs, {
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    child.stdout.on("data", (d) => (stdout += d));
    child.on("close", (code) => {
      if (code !== 0) reject(new Error(`claude exited with code ${code}`));
      else resolve(stdout);
    });
  });
}
```

### Option B: Anthropic SDK

Call the API directly. Requires `pnpm add @anthropic-ai/sdk` and an `ANTHROPIC_API_KEY`.

```ts
// handlers/lib.ts
import Anthropic from "@anthropic-ai/sdk";

const client = new Anthropic();

export async function callClaude(args: {
  prompt: string;
}): Promise<string> {
  const response = await client.messages.create({
    model: "claude-sonnet-4-20250514",
    max_tokens: 4096,
    messages: [{ role: "user", content: args.prompt }],
  });
  return response.content
    .filter((block) => block.type === "text")
    .map((block) => block.text)
    .join("");
}
```

### Update the handler

With either option, replace the stub in `migrateComponent`:

```ts
export const migrateComponent = createHandler({
  inputValidator: z.string(),
  handle: async ({ value: file }) => {
    await callClaude({
      prompt: `Migrate ${file} from class-based React components to functional components using hooks. Preserve all behavior exactly.`,
      allowedTools: ["Read", "Edit"],  // CLI only — SDK doesn't use tools this way
    });
  },
}, "migrateComponent");
```

Run it again:

```bash
pnpm exec tsx run.ts
```

Each file gets its own Claude instance, running in parallel.

## Next steps

- [Patterns](../patterns/) — looping, branching, error handling, timeouts, racing, and more
- [Repertoire](../repertoire/) — real-world workflow examples
- [Builtins reference](../reference/builtins) — every combinator with type signatures
- [Demos](https://github.com/barnum-circus/barnum/tree/master/demos) — complete working examples
