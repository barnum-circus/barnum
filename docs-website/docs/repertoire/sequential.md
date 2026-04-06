---
image: /img/og/repertoire-sequential.png
---

# Sequential Processing

Process items one at a time, in order. Use this when each step must see the result of the previous one — for example, applying multiple database migrations or making ordered changes to the same file.

## Pattern

Write a handler that iterates through items sequentially:

```ts
export const processSequentially = createHandler({
  inputValidator: z.array(z.string()),
  handle: async ({ value: files }) => {
    for (const file of files) {
      await callClaude({
        prompt: `Apply the next migration to ${file}.`,
        allowedTools: ["Read", "Edit"],
      });
    }
  },
}, "processSequentially");
```

## Example

Apply database migrations in order, where each migration depends on the schema state left by the previous one:

```ts
export const listMigrations = createHandler({
  outputValidator: z.array(z.string()),
  handle: async () => {
    return readdirSync("migrations")
      .filter((f) => f.endsWith(".sql"))
      .sort();
  },
}, "listMigrations");

export const applyMigrations = createHandler({
  inputValidator: z.array(z.string()),
  handle: async ({ value: migrations }) => {
    for (const migration of migrations) {
      await callClaude({
        prompt: `Apply the SQL migration in migrations/${migration}. Run it against the database and verify it succeeded.`,
        allowedTools: ["Read", "Bash"],
      });
    }
  },
}, "applyMigrations");
```

```ts
await workflowBuilder()
  .workflow(() => pipe(listMigrations, applyMigrations))
  .run();
```

## Key points

- Use `forEach` when items can be processed in parallel (independent work).
- Use a sequential handler when order matters (dependent work).
- The handler itself manages the iteration — Barnum doesn't need a special combinator for this.
