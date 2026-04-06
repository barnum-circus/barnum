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

## Write a Claude helper

Barnum handlers call Claude via the CLI. Here's a minimal helper using `child_process.spawn`:

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

You can also test prompts directly from the command line:

```bash
claude -p "Migrate this React class component to a functional component with hooks." \
  --allowedTools Read Edit
```

## Write handlers

```ts
// handlers/steps.ts
import { createHandler } from "@barnum/barnum";
import { z } from "zod";
import { readdirSync } from "fs";
import { callClaude } from "./lib.js";

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
  handle: async ({ value: file }) => {
    await callClaude({
      prompt: `Migrate ${file} from class-based React components to functional components using hooks. Preserve all behavior exactly.`,
      allowedTools: ["Read", "Edit"],
    });
  },
}, "migrateComponent");
```

Each handler is an async function with optional Zod validators for input and output. Handlers run in isolated subprocesses — each one only sees its own input, never the full workflow.

## Compose a workflow

```ts
// run.ts
import { workflowBuilder } from "@barnum/barnum";
import { listFiles, migrateComponent } from "./handlers/steps.js";

await workflowBuilder()
  .workflow(() =>
    listFiles
      .forEach(migrateComponent)
      .drop()
  )
  .run();
```

`listFiles` returns an array of file paths. `forEach` fans out — each file flows through `migrateComponent` in parallel, with a separate Claude instance per file.

## Run it

```bash
pnpm exec tsx run.ts
```

## Next steps

- See the [demos](https://github.com/barnum-circus/barnum/tree/master/demos) for complete working examples
- Read the [introduction](./index.md) for the full narrative on why Barnum exists
- Check out the [builtins reference](./reference/builtins.md) to see what combinators Barnum gives you
