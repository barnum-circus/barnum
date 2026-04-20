# Scan and Splits

Concrete implementations for: scan, Iterator.splitFirst, Iterator.splitLast, splitFirstN, splitLastN. Plus a demo showing splitFirst as a sequential iteration pattern.

---

## 1. Scan — composed from loop + splitFirst

`scan(init, body)`: `Iterator<T> → Iterator<TAcc>` where `body: [TAcc, T] → TAcc`.

Threads an accumulator through elements one at a time, collecting each accumulator value. The accumulator IS the output — no separate output type. Composed from `loop` + `splitFirst` + `bindInput`. No new AST nodes.

### Implementation

```typescript
// In Iterator namespace:
scan<TElement, TAcc>(
  init: Pipeable<void, TAcc>,
  body: Pipeable<[TAcc, TElement], TAcc>,
): TypedAction<IteratorT<TElement>, IteratorT<TAcc>> {
  return Iterator.collect<TElement>()        // Iterator<T> → T[]
    .then(bindInput<TElement[]>(elements =>
      all(init, elements, constant<TAcc[]>([]))
        // State: [TAcc, TElement[], TAcc[]], Done: TAcc[]
        .then(loop<TAcc[], [TAcc, TElement[], TAcc[]]>((recur, done) =>
          bindInput<[TAcc, TElement[], TAcc[]]>(state => {
            const acc = state.getIndex(0).unwrap();
            const remaining = state.getIndex(1).unwrap();
            const outputs = state.getIndex(2).unwrap();

            return remaining.splitFirst().branch({
              None: outputs.then(done),
              Some: bindInput<[TElement, TElement[]]>(headTail => {
                const head = headTail.getIndex(0).unwrap();
                const tail = headTail.getIndex(1).unwrap();

                return all(acc, head)            // [TAcc, TElement]
                  .then(body)                    // TAcc
                  .then(bindInput<TAcc>(newAcc => {
                    const newOutputs = all(outputs, newAcc.then(wrapInArray<TAcc>()))
                      .then(flatten<TAcc>());
                    return all(newAcc, tail, newOutputs).then(recur);
                  }));
              }),
            });
          }),
        )),
    ))
    .then(Iterator.fromArray<TAcc>()) as TypedAction<IteratorT<TElement>, IteratorT<TAcc>>;
},
```

State threaded through loop: `[TAcc, TElement[], TAcc[]]` — accumulator, remaining elements, collected outputs. Each iteration: `splitFirst(remaining)` → `None` means done, `Some([head, tail])` means run body (returns new accumulator), append it to outputs, recur. Array append via `all(existing, wrapInArray(new)).flatten()`.

---

## 2. Iterator.splitFirst

`Iterator<T> → Option<[T, Iterator<T>]>`

Composes from existing `SplitFirst` builtin.

### Implementation

```typescript
splitFirst<TElement>(): TypedAction<
  IteratorT<TElement>,
  OptionT<[TElement, IteratorT<TElement>]>
> {
  return Iterator.collect<TElement>()                           // Iterator<T> → T[]
    .then(splitFirst())                                         // → Option<[T, T[]]>
    .then(Option.map(
      all(
        getIndex(0).unwrap(),                                    // → T
        getIndex(1).unwrap().then(Iterator.fromArray()),         // → Iterator<T>
      ),
    )) as TypedAction<IteratorT<TElement>, OptionT<[TElement, IteratorT<TElement>]>>;
},
```

Postfix `.splitFirst()` changes from direct builtin call to branchFamily dispatch:

```typescript
function splitFirstMethod(this: TypedAction): TypedAction {
  return this.then(branchFamily({
    Iterator: IteratorNs.splitFirst(),
    Array: splitFirst(),
  }));
}
```

---

## 3. Iterator.splitLast

`Iterator<T> → Option<[Iterator<T>, T]>`

### Implementation

```typescript
splitLast<TElement>(): TypedAction<
  IteratorT<TElement>,
  OptionT<[IteratorT<TElement>, TElement]>
> {
  return Iterator.collect<TElement>()                           // Iterator<T> → T[]
    .then(splitLast())                                          // → Option<[T[], T]>
    .then(Option.map(
      all(
        getIndex(0).unwrap().then(Iterator.fromArray()),         // → Iterator<T>
        getIndex(1).unwrap(),                                    // → T
      ),
    )) as TypedAction<IteratorT<TElement>, OptionT<[IteratorT<TElement>, TElement]>>;
},
```

Postfix: same branchFamily pattern as splitFirst.

---

## 4. splitFirstN / splitLastN — new builtins

### splitFirstN

`T[] → [T[], T[]]` — splits at position n from the start.

```typescript
// BuiltinKind:
| { kind: "SplitFirstN"; n: number }

// Rust: (input[..n.min(len)], input[n.min(len)..])

// Constructor in builtins/array.ts:
export function splitFirstN<TElement>(
  n: number,
): TypedAction<TElement[], [TElement[], TElement[]]> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "SplitFirstN", n } },
  });
}
```

### splitLastN

`T[] → [T[], T[]]` — splits at position n from the end.

```typescript
// BuiltinKind:
| { kind: "SplitLastN"; n: number }

// Rust: let split = len.saturating_sub(n); (input[..split], input[split..])

// Constructor in builtins/array.ts:
export function splitLastN<TElement>(
  n: number,
): TypedAction<TElement[], [TElement[], TElement[]]> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "SplitLastN", n } },
  });
}
```

Both clamp — `splitFirstN(100)` on `[1,2,3]` → `[[1,2,3], []]`.

---

## 5. Demo: sequential iteration with splitFirst

`splitFirst` + `loop` + `branch` processes elements one at a time in order — the sequential counterpart to `.iterate().map()` (parallel via ForEach).

### The pattern

```
loop:
  splitFirst(iterator) → Option<[head, tail]>
  None → done
  Some → bindInput(pair =>
    process(pair[0]),   // head — sequential side effect
    recur(pair[1])      // tail — continue
  )
```

`bindInput` is essential — it captures the `[head, tail]` pair so both components remain accessible after the head is consumed by a handler.

### Demo: deploy services in dependency order

Services must be deployed one at a time in dependency order. Each deployment must complete (and be verified) before the next one starts. This is the classic case where `.iterate().map(deploy)` would break things — parallel deploys would violate dependency ordering.

**`demos/sequential-deploy/handlers/deploy.ts`:**

```typescript
import { createHandler } from "@barnum/barnum/runtime";
import { z } from "zod";

/** Return the services in dependency order. First must deploy before second, etc. */
export const getServices = createHandler(
  {
    outputValidator: z.array(z.string()),
    handle: async () => {
      console.error("[getServices] Resolving dependency order...");
      return ["database", "cache", "auth", "api", "frontend"];
    },
  },
  "getServices",
);

/** Deploy a single service. Takes the service name, returns the service name. */
export const deployService = createHandler(
  {
    inputValidator: z.string(),
    outputValidator: z.string(),
    handle: async ({ value: service }) => {
      const delay = 500 + Math.floor(Math.random() * 1000);
      console.error(`[deploy] Deploying ${service}...`);
      await new Promise((resolve) => setTimeout(resolve, delay));
      console.error(`[deploy] ${service} deployed (${delay}ms)`);
      return service;
    },
  },
  "deployService",
);

/** Verify a service is healthy after deployment. Takes the service name, returns the service name. */
export const verifyService = createHandler(
  {
    inputValidator: z.string(),
    outputValidator: z.string(),
    handle: async ({ value: service }) => {
      console.error(`[verify] Health-checking ${service}...`);
      await new Promise((resolve) => setTimeout(resolve, 200));
      console.error(`[verify] ${service} healthy`);
      return service;
    },
  },
  "verifyService",
);
```

**`demos/sequential-deploy/run.ts`:**

```typescript
/**
 * Sequential deploy demo: deploy services one at a time in dependency order.
 *
 * Uses the splitFirst + loop + branch pattern for sequential iteration.
 * Each service is fully deployed and verified before the next one starts.
 *
 * Contrast with `.iterate().map(deploy)` which would deploy all services
 * concurrently — violating dependency ordering.
 *
 * Usage: pnpm exec tsx run.ts
 */

import {
  runPipeline,
  pipe,
  loop,
  drop,
  identity,
  Iterator,
  bindInput,
} from "@barnum/barnum/pipeline";
import { getServices, deployService, verifyService } from "./handlers/deploy";

console.error("=== Sequential deploy demo ===\n");

runPipeline(
  pipe(
    getServices,                             // → string[]
    identity<string[]>().iterate(),          // → Iterator<string>

    // Process one service at a time in order
    loop<void, Iterator<string>>((recur, done) =>
      Iterator.splitFirst<string>().branch({
        // All services deployed
        None: done,

        // Deploy head, then continue with tail
        Some: bindInput<[string, Iterator<string>]>(pair =>
          pipe(
            pair.getIndex(0).unwrap(),       // current service name
            deployService,                   // deploy it (waits for completion)
            verifyService,                   // verify it's healthy
            drop,                            // discard service name
            pair.getIndex(1).unwrap(),       // remaining services
            recur,                           // next service
          ),
        ),
      }),
    ),
  ),
);
```

Expected output:

```
=== Sequential deploy demo ===

[getServices] Resolving dependency order...
[deploy] Deploying database...
[deploy] database deployed (732ms)
[verify] Health-checking database...
[verify] database healthy
[deploy] Deploying cache...
[deploy] cache deployed (1203ms)
[verify] Health-checking cache...
[verify] cache healthy
[deploy] Deploying auth...
...
```

Each service completes before the next starts. With `.iterate().map(pipe(deployService, verifyService))`, all five would deploy concurrently — the frontend would try to start before the database is ready.

---

## 6. Implementation order

### Phase A: splitFirstN / splitLastN builtins (Rust + TypeScript)

1. Add `SplitFirstN`, `SplitLastN` to Rust `BuiltinKind`
2. Add TypeScript constructors to `builtins/array.ts`

### Phase B: Iterator.splitFirst / Iterator.splitLast (TypeScript only)

1. Add to iterator.ts
2. Change postfix to branchFamily dispatch

### Phase C: Iterator.scan (TypeScript only)

Depends on Phase B.

1. Add to iterator.ts (loop + splitFirst composition)
2. Wire postfix method

### Phase D: Sequential processing demo

Depends on Phase B.
