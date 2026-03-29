# Retry, Timeouts, and Error Handling

How to handle handler failures, bad output, and retry logic.

## The two failure modes

See RUNTIME_TYPE_CHECKING.md for full details. The key distinction:

| Failure type | Example | Handler behavior | Engine response |
|---|---|---|---|
| **Byzantine fault** | Deterministic handler returns wrong type | Worker SDK sends `ContractViolation` IPC | `Failed(ByzantineFault)`, kill workflow |
| **Domain failure** | LLM returns malformed JSON, API rate-limits | Handler catches error, returns `Result` `Err` | Routes normally; AST handles retry |

Byzantine faults are not retryable. The AST's type guarantees are broken. The workflow is dead.

Domain failures are data. They flow through the AST as `Result` variants. The AST retries via `loop`/`branch` or algebraic effects (`tryCatch`/`Perform(Throw)`).

## Domain failures: explicit Result returns

Handlers that call non-deterministic sources (LLMs, APIs, shell commands) should catch errors internally and return `Result`:

```ts
export const callApi = createHandler({
  inputValidator: z.object({ url: z.string() }),
  handle: async ({ value }): Promise<Result<ApiResponse, string>> => {
    try {
      const response = await fetch(value.url);
      const data = await response.json();
      return { kind: "Ok", value: data };
    } catch (e) {
      return { kind: "Err", value: e.message };
    }
  },
}, "callApi");
```

The handler decides what constitutes an error. Full type safety. The handler can include diagnostic information in the `Err` payload (raw LLM output, HTTP status, etc.) that downstream retry logic can use.

## Retry patterns

### Simple retry via loop + branch

If the handler models failure as a Result, retry is just loop + branch:

```ts
loop(
  pipe(
    callApi,
    branch({
      Ok: done(),
      Err: recur(),  // retry — discards error, re-runs with same input
    }),
  ),
)
```

This retries forever. To limit retries, thread a counter or use algebraic effects.

### Retry with error feedback (LLM self-correction)

For LLM handlers, the error payload often contains the raw output and the parse failure. Feed this back into the prompt:

```ts
loop(
  pipe(
    askAgent,
    branch({
      Ok: done(),
      Err: pipe(augmentPromptWithFeedback, recur()),
    }),
  ),
)
```

The `augmentPromptWithFeedback` handler appends the error message to the prompt context, giving the LLM a chance to correct its output.

### Retry with counter (current primitives)

Thread a `{ attempt: number, ... }` through the pipeline:

```ts
pipe(
  augment(constant({ attempt: 0 })),
  loop(
    pipe(
      augment(pipe(
        extractField("attempt"),
        increment,
      )),
      // ... this gets ugly fast
    ),
  ),
)
```

This is painful. The counter threading pollutes the entire pipeline. Better options below.

### Retry as a combinator (new primitive)

A cleaner API: `retry(action, { maxAttempts: 3 })` as a new combinator.

```ts
retry(riskyHandler, { maxAttempts: 3 })
// Returns: Ok(result) | Err(lastError)
```

**Option A: Sugar over loop (userland)** — Doable but ugly without let-bindings or context.

**Option B: New AST node (Rust-side)** — `{ kind: "Retry", action: Action, maxAttempts: number }`. The Rust scheduler handles the retry loop internally with an attempt counter. Much cleaner.

**Recommendation**: Option B. Retry is common enough and hard enough in userland that it warrants scheduler support.

### Retry via algebraic effects

With algebraic effects (see EFFECTS_PHASE_3_TRYCATCH.md), retry becomes:

```ts
loop((recur, done) =>
  tryCatch(
    pipe(invokeWithThrow(askAgent), done()),
    (schemaError) => pipe(
      augmentPromptWithFeedback(schemaError),
      recur(),
    ),
  ),
)
```

The handler returns `{ kind: "Err", ... }` which `invokeWithThrow` converts to `Perform(Throw)`. The `tryCatch` catches it and the loop retries.

## Output validation

### The problem

Handlers declare `inputValidator` (zod schema) but not `outputValidator`. If a handler returns `{ naem: "typo" }` instead of `{ name: "value" }`, the error surfaces in the *next* handler's input validation — or worse, propagates silently.

### Solution: add `outputValidator` to createHandler

```ts
export const myHandler = createHandler({
  inputValidator: z.object({ file: z.string() }),
  outputValidator: z.object({ content: z.string() }),
  handle: async ({ value }) => {
    return { content: readFile(value.file) };
  },
});
```

The worker SDK validates the output before sending the IPC payload:

```ts
const result = await handler.__definition.handle({ value: input.value });

if (handler.__definition.outputValidator) {
  try {
    handler.__definition.outputValidator.parse(result);
  } catch (err) {
    // Deterministic handler violated its contract.
    sendIPC({ kind: "ContractViolation", handler: handler.name, error: err.message });
    return;
  }
}

sendIPC({ kind: "Yield", value: result });
```

### When to use `outputValidator` vs internal Result returns

- **Deterministic handlers** (pure transforms): Use `outputValidator`. A schema violation is a bug → `ContractViolation` → kill workflow.
- **Non-deterministic handlers** (LLMs, APIs): Do NOT use `outputValidator`. Catch Zod errors inside the handler, return `Result`. A schema violation is expected → data flows through AST → retry.

### Should output validators be inferred?

No — TypeScript types are erased at runtime. Zod schemas are runtime objects. You can't automatically bridge them. The user must explicitly provide the zod schema.

However, TypeScript can **type-check** that the output validator matches the handle function's return type:

```ts
export function createHandler<TValue, TOutput>(definition: {
  inputValidator?: z.ZodType<TValue>;
  outputValidator?: z.ZodType<TOutput>;  // Must match handle's return type
  handle: (context: { value: TValue }) => Promise<TOutput>;
}): Handler<TValue, TOutput>;
```

## Timeouts

### Handler-level timeout

The Rust scheduler spawns handler subprocesses. It can enforce a timeout per handler invocation.

```ts
export const slowHandler = createHandler({
  timeout: 60_000,  // 60 seconds
  handle: async ({ value }) => { ... },
});
```

If the handler times out, the scheduler treats it as a failure. For deterministic handlers, this is a `ContractViolation` (the handler is broken). For non-deterministic handlers wrapped in `tryAction`, this produces an `Err` result.

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

## How these compose

```ts
// Deterministic handler with output validation:
// If it fails → ContractViolation → workflow dies
export const deriveBranch = createHandler({
  inputValidator: z.object({ description: z.string() }),
  outputValidator: z.object({ branch: z.string() }),
  handle: async ({ value }) => ({
    branch: value.description.toLowerCase().replace(/[^a-z0-9]+/g, "-").slice(0, 40),
  }),
});

// Non-deterministic handler with internal error handling:
// If it fails → Result.err → AST retries
export const askAgent = createHandler({
  inputValidator: z.object({ prompt: z.string() }),
  // No outputValidator — handler manages its own errors
  handle: async ({ value }): Promise<Result<AgentOutput, string>> => {
    try {
      const raw = await llm.generate(value.prompt);
      return { kind: "Ok", value: AgentOutputSchema.parse(JSON.parse(raw)) };
    } catch (err) {
      return { kind: "Err", value: err.message };
    }
  },
});

// Pipeline: retry the agent, use the deterministic transform
pipe(
  loop(pipe(
    askAgent,
    branch({ Ok: done(), Err: recur() }),
  )),
  deriveBranch,
)
```

## Priority

1. **Output validation** — low effort, high value. Add optional `outputValidator` to handler definition, validate in worker SDK, send `ContractViolation` on failure.
2. **Timeouts** — medium effort. Rust-side change to add timeout to subprocess spawning.
3. **Retry combinator** — depends on `Result` type existing. As an AST node (Option B), medium effort.
