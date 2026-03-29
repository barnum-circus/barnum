# Retry, Timeouts, and Output Validation

How to handle handler failures, bad output, and retry logic.

## The problem space

Handlers can fail in several ways:

1. **Handler throws** — process exits non-zero (currently fatal)
2. **Handler returns wrong shape** — downstream handler gets garbage input, fails with a confusing error far from the source
3. **Handler times out** — subprocess hangs forever (no timeout mechanism exists)
4. **Handler returns a domain-level error** — e.g., Claude API returns "rate limited" (handler can model this as a Result)

Each failure mode needs a different response.

## Retry in userland (no AST changes)

If the handler models failure as a Result, retry is just loop + branch:

```ts
loop(
  pipe(
    tryAction(handler),
    branch({
      Ok: done(),
      Err: recur(),
    }),
  ),
)
```

This retries forever. To limit retries, we need a counter. Without context/variables, the counter must be threaded through the pipeline.

### Retry with counter (current primitives)

Thread a `{ attempt: number, ... }` through the pipeline:

```ts
pipe(
  // Start: wrap input with attempt counter
  augment(constant({ attempt: 0 })),

  loop(
    pipe(
      // Increment attempt
      augment(pipe(
        extractField("attempt"),
        increment,  // handler: n => n + 1
        tag("attempt"),  // wait, this doesn't work right
      )),

      // ... this gets ugly fast
    ),
  ),
)
```

This is painful. The counter threading pollutes the entire pipeline.

### Retry as a combinator (new primitive)

A cleaner API: `retry(action, { maxAttempts: 3 })` as a new combinator.

```ts
retry(riskyHandler, { maxAttempts: 3 })
// Equivalent to: run handler, if it fails, retry up to 3 times
// Returns: Ok(result) | Err(lastError)
```

Implementation options:

**Option A: Sugar over loop + tryAction (userland)**

```ts
function retry<TIn, TOut>(
  action: TypedAction<TIn, TOut>,
  config: { maxAttempts: number },
): TypedAction<TIn, Result<TOut, Error>> {
  // Would need: counter threading, tryAction, loop, branch
  // Requires context/variables to avoid threading the counter
}
```

This is doable but ugly without context/variables.

**Option B: New AST node (Rust-side)**

```ts
{ kind: "Retry", action: Action, maxAttempts: number }
```

The Rust scheduler handles the retry loop internally, with access to an attempt counter. This is much cleaner — the retry semantics live in the scheduler, not in the AST.

**Recommendation**: Option B. Retry is common enough and hard enough in userland that it warrants scheduler support. The Rust side already wraps handler execution — adding a retry loop around that wrapper is straightforward.

## Output validation

### The problem

Handlers declare `inputValidator` (zod schema) but not `outputValidator`. If a handler returns `{ naem: "typo" }` instead of `{ name: "value" }`, the error surfaces in the *next* handler's input validation — or worse, propagates silently.

### Solution: add `outputValidator` to HandlerDefinition

```ts
export const myHandler = createHandler({
  inputValidator: z.object({ file: z.string() }),
  outputValidator: z.object({ content: z.string() }),
  handle: async ({ value }) => {
    // ... if this returns wrong shape, outputValidator catches it
  },
});
```

The worker validates the output before sending it to stdout:

```ts
// worker.ts
const result = await handler.__definition.handle({ value: input.value });

if (handler.__definition.outputValidator) {
  handler.__definition.outputValidator.parse(result);  // throws on mismatch
}

process.stdout.write(JSON.stringify(result));
```

### Should output validators be inferred?

The `handle` function's return type is known at compile time. Could we auto-generate a zod validator from the TypeScript type?

No — TypeScript types are erased at runtime. Zod schemas are runtime objects. You can't automatically bridge them. The user must explicitly provide the zod schema.

However, we can **type-check** that the output validator matches the handle function's return type:

```ts
export function createHandler<TValue, TOutput>(definition: {
  inputValidator: z.ZodType<TValue>;
  outputValidator?: z.ZodType<TOutput>;  // Must match handle's return type
  handle: (context: { value: TValue }) => Promise<TOutput>;
}): Handler<TValue, TOutput>;
```

If the user provides an `outputValidator` that doesn't match the return type, TypeScript rejects it at compile time.

### Is output validation always needed?

For handlers that call Claude (non-deterministic output), output validation is critical — Claude might return malformed JSON, extra fields, or missing fields.

For pure data transforms (`classifyErrors`, `deriveBranch`), the TypeScript compiler already ensures correctness. Output validation is redundant.

**Recommendation**: Optional `outputValidator`. Strongly encouraged for handlers that invoke external processes (Claude, shell commands, APIs). Not needed for pure transforms.

## Timeouts

### Handler-level timeout

The Rust scheduler spawns handler subprocesses. It can enforce a timeout per handler invocation.

**Implementation**: Add `timeout_ms` to the handler config or step config. The Rust side uses `wait_timeout` on the child process.

```ts
export const slowHandler = createHandler({
  timeout: 60_000,  // 60 seconds
  handle: async ({ value }) => { ... },
});
```

If the handler times out, the scheduler treats it as a failure (same as non-zero exit). Combined with `tryAction`, this produces an Err result.

### Workflow-level timeout

The Rust CLI could accept a `--timeout` flag for the entire workflow.

```bash
barnum run --config ... --timeout 300000  # 5 minutes
```

### Step-level timeout (via step config)

Named steps could have timeout metadata:

```ts
.registerSteps({
  SlowStep: { action: handler, timeout: 60_000 },
})
```

This requires extending the step registration API to accept metadata alongside the action.

## Catching type errors at runtime

### The handler boundary

The handler subprocess is the trust boundary. Input comes from the scheduler (trusted) or from the previous handler's output (semi-trusted). Output goes to the scheduler for the next handler.

Currently, `inputValidator` runs in the worker before calling `handle()`. Adding `outputValidator` closes the loop.

### The scheduler boundary

The Rust scheduler receives handler output as JSON. It could validate the JSON against a schema before routing it to the next handler. This requires shipping zod schemas (or JSON Schema equivalents) to the Rust side.

**Recommendation**: Keep validation in the TypeScript worker. The Rust side is a transport layer — it shouldn't know about output shapes. Validation at the TypeScript handler boundary is sufficient.

## How `tryAction` + retry + output validation compose

```ts
// Pattern: resilient handler with retry, timeout, and output validation
retry(
  tryAction(riskyHandler),  // wraps in Result
  { maxAttempts: 3 },
)

// riskyHandler has:
//   outputValidator: z.object({ content: z.string() })
//   timeout: 30_000
//
// If handler throws → tryAction catches → Err → retry
// If handler times out → tryAction catches → Err → retry
// If output invalid → outputValidator throws → tryAction catches → Err → retry
// If output valid → Ok(result)
// After 3 failures → Err(lastError) propagates
```

## Priority

1. **Output validation** — low effort, high value. Just add optional `outputValidator` to handler definition and validate in worker.ts.
2. **Timeouts** — medium effort. Rust-side change to add timeout to subprocess spawning.
3. **Retry** — depends on `tryAction` existing first. As an AST node (Option B), medium effort.
