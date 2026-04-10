# Effect: Mutable State

## The API

```ts
withState<TState, TIn, TOut>(
  initial: TState,
  body: (ctx: {
    get: () => TypedAction<never, TState>;
    set: () => TypedAction<TState, void>;
    update: (action: Pipeable<TState, TState>) => TypedAction<never, void>;
  }) => Pipeable<TIn, TOut>,
): TypedAction<TIn, TOut>
```

HOAS pattern — same as `scope`, `loop`, `bind`. The callback receives effect tokens scoped to this state cell.

- **`get()`** — read the current state. Input is `never` (doesn't consume pipeline value). Output is `TState`.
- **`set()`** — write a new state. Input is `TState` (the new value). Output is `void`.
- **`update(action)`** — apply a transformation to the current state. Input is `never`. Output is `void`. The `action` receives the current state and produces the new state.

### Example: accumulator

```ts
withState(0, ({ get, set, update }) =>
  pipe(
    forEach(
      pipe(
        processItem,
        update(addOne),   // increment counter after each item
      ),
    ).drop(),
    get(),                // read final count
  ),
)
```

### Example: building a result map

```ts
withState<Record<string, Result>>({}, ({ get, set, update }) =>
  pipe(
    forEach(
      bindInput<Task>((task) =>
        pipe(
          task.then(execute),
          // merge this result into the accumulator
          update(
            bindInput<Record<string, Result>>((currentState) =>
              pipe(all(currentState, task.get("id"), ...), mergeResult)
            ),
          ),
        ),
      ),
    ).drop(),
    get(),
  ),
)
```

## How it compiles

Two effects: `getEffect` and `setEffect`. Two nested Handles carrying shared state.

```ts
// User writes:
withState(initialValue, ({ get, set }) => body)

// Compiles to:
Chain(
  All(Constant(initialValue), Identity),     // state slot + pipeline input
  Handle(getEffect, getHandler,
    Handle(setEffect, setHandler,
      Chain(GetIndex(1), body)            // body receives pipeline input
    )
  )
)
```

The Handle frame's state is `[TState, TIn]`. The pipeline input is preserved in slot 1 and extracted for the body. The state cell lives in slot 0.

### Handler DAGs

**getHandler** — reads `state[0]`, resumes with it:

```
Input: { payload: void, state: [TState, TIn] }
Output: { kind: "Resume", value: state[0], state_update: { kind: "Unchanged" } }
```

AST: `GetField("state") → GetIndex(0) → Tag("Resume")`

Wait — this needs to produce `{ kind: "Resume", value: <V>, state_update: { kind: "Unchanged" } }`. The handler DAG needs to construct this object. In bind, the handler is `GetField("state") → GetIndex(n) → Tag("Resume")` which produces `{ kind: "Resume", value: state[n] }`. The engine interprets a missing `state_update` as `Unchanged`.

**setHandler** — writes `payload` as the new `state[0]`, resumes with void:

```
Input: { payload: TState, state: [TState, TIn] }
Output: { kind: "Resume", value: void, state_update: { kind: "Updated", value: [payload, state[1]] } }
```

This is more involved. The handler must:
1. Extract `payload` (the new state)
2. Extract `state[1]` (preserve pipeline input)
3. Construct `[payload, state[1]]` as the new state
4. Tag as Resume with Updated state

AST: `All(GetField("payload"), Chain(GetField("state"), GetIndex(1))) → Tag("Resume")` with `state_update` constructed from the All output.

The exact AST depends on how `state_update` is wired. If the engine convention is that Resume's value is delivered to the body and the state_update is a separate field, the handler needs to produce the full `{ kind, value, state_update }` object. This is an implementation detail for the engine integration.

**updateHandler** — applies a transform action to `state[0]`:

`update(action)` compiles differently from `get`/`set`. It mints a third effect (`updateEffect`) whose handler:
1. Extracts `state[0]`
2. Runs `action` on it (the handler DAG includes the user's action)
3. Constructs `[result, state[1]]` as the new state
4. Tags as Resume with Updated state

Alternatively, `update(action)` desugars in TypeScript to `pipe(get(), action, set())` — three Performs, no new handler needed. This is simpler but non-atomic (another concurrent branch could interleave between the get and set).

## State laws

In algebraic effects literature, the State effect satisfies these laws:

1. **Get-Put**: `get() → set()` ≡ no-op (writing back what you just read does nothing)
2. **Put-Get**: `set(s) → get()` yields `s` (what you write is what you read)
3. **Put-Put**: `set(s1) → set(s2)` ≡ `set(s2)` (last write wins)
4. **Get-Get**: `get() → get()` yields the same value both times (reads are idempotent)

### These laws do NOT hold in barnum

Barnum actions are asynchronous. Handlers are LLM calls, API calls, shell commands. Multiple branches of `all()` execute concurrently. There is no synchronous execution guarantee.

**Get-Get breaks under concurrency:**
```ts
all(get(), get())
// Both run concurrently. If another effect modifies state
// between them, they return different values.
```

**Put-Put is nondeterministic under concurrency:**
```ts
all(set(a), set(b))
// Both run concurrently. Order is nondeterministic.
// Which write wins? Engine-dependent.
```

**Put-Get breaks across async boundaries:**
```ts
pipe(set(s), someAsyncAction, get())
// If another concurrent branch modifies state while
// someAsyncAction runs, get() may not return s.
```

### What DOES hold

**Sequential pipelines are ordered.** Within a single sequential pipeline (no `all()`), effects are ordered. `pipe(set(s), get())` returns `s` because `set` completes before `get` fires. The Perform/Resume protocol ensures this — the body only advances after the handler resumes.

**Each effect is atomic.** A single `get()` or `set()` or `update()` is a single Perform → handler → Resume cycle. The handler runs as a unit. No interleaving within a single effect operation.

**State is scoped.** Nested `withState` creates isolated state cells — inner state doesn't alias outer state. Each Handle frame has its own state.

**`update` is atomic if implemented as a single effect** (not desugared to get+set). The handler reads the current state, applies the transform, and writes the result in one handler invocation. No window for interleaving.

### Practical implications

State in barnum is a **shared mutable cell**, not algebraic state. Think `Mutex<T>`, not the State monad. Within a sequential pipeline, it behaves like the State monad. Across concurrent branches (`all`, `forEach`), it's a race.

If you need concurrent-safe accumulation, use `update(action)` (single atomic effect) rather than `pipe(get(), transform, set())` (three effects with interleaving risk).

If you need truly isolated state per concurrent branch, put `withState` inside the `forEach` body, not outside it.

## Relationship to bind

`bind` captures immutable values. `withState` maintains a mutable cell. They're complementary:

- `bind` = multiple read-only values, each in its own Handle with ReadVar
- `withState` = one read-write cell in a Handle with Get/Set handlers

Both compile to Handle/Perform. Both use HOAS to scope the effect tokens.

## Files to change

| File | What changes |
|------|-------------|
| `libs/barnum/src/builtins.ts` | Add `withState()` function |
| `libs/barnum/src/ast.ts` | Re-export from ast barrel if needed |
| `libs/barnum/tests/types.test.ts` | Type-level tests |
| `libs/barnum/tests/patterns.test.ts` | AST shape tests |
| Rust engine | Handle frames already support state + state_update. No engine changes needed — only new handler DAGs. |
