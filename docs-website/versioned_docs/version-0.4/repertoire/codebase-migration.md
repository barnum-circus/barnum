# Codebase Migration

Convert an entire codebase from one pattern to another — JavaScript to TypeScript, class components to hooks, CommonJS to ESM — processing each file independently and fixing cross-file type errors at the end.

## Workflow

From [`demos/convert-folder-to-ts`](https://github.com/barnum-circus/barnum/tree/master/demos/convert-folder-to-ts):

```ts
runPipeline(
  setup
    .then(
      listFiles.iterate().map(migrate({ to: "Typescript" })).collect().drop(),
    )
    .then(typeCheckFix),
);
```

## Stages

1. **Setup** — determine input and output directories.
2. **List files** — find all files to migrate.
3. **Migrate each file** — `.iterate().map()` processes every file concurrently. Each migration agent receives a single file and the target format — it doesn't know about other files.
4. **Type-check/fix loop** — after all files are migrated, run the compiler and fix errors in a loop:

```ts
export const typeCheckFix = loop((recur) =>
  typeCheck.then(classifyErrors).branch({
    HasErrors: Iterator.fromArray<TypeError>().map(fix).drop().then(recur),
    Clean: drop,
  }),
);
```

## Key points

- Per-file migration runs in parallel — the agent migrating `utils.ts` doesn't see `api.ts`.
- The type-check/fix loop runs after all migrations, catching cross-file issues (changed exports, missing types).
- `createHandlerWithConfig` lets `migrate` accept a step config (`{ to: "Typescript" }`) separate from the per-file input, keeping the handler reusable across migration types.
- The same structure works for any file-by-file transformation: just swap the `migrate` handler.
