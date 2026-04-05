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

Create a `handlers/steps.ts` file with your handler functions:

```ts
import { createHandler } from "@barnum/barnum";
import { z } from "zod";
import { readdirSync } from "fs";

export const listFiles = createHandler({
  outputValidator: z.array(z.string()),
  handle: async () => {
    return readdirSync("src/").filter(f => f.endsWith(".ts"));
  },
}, "listFiles");

export const processFile = createHandler({
  inputValidator: z.string(),
  handle: async ({ value: file }) => {
    console.log(`Processing ${file}`);
  },
}, "processFile");
```

Handlers are async functions with optional Zod validators for input and output. They run in isolated subprocesses — each handler only sees its own input, never the full workflow.

## Compose a workflow

Create `run.ts` to compose your handlers into a workflow:

```ts
import { workflowBuilder, pipe } from "@barnum/barnum";
import { listFiles, processFile } from "./handlers/steps.js";

await workflowBuilder()
  .workflow(() =>
    listFiles
      .forEach(processFile)
      .drop()
  )
  .run();
```

`listFiles` returns an array. `forEach` fans out — each element flows through `processFile` in parallel.

## Run it

```bash
pnpm exec tsx run.ts
```

## Next steps

- See the [demos](https://github.com/barnum-circus/barnum/tree/master/demos) for complete working examples
- Read the [Introduction](./index.md) for the full narrative on why Barnum exists
