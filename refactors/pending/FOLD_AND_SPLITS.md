# Fold and Splits

Concrete implementations for: fold, Iterator.splitFirst, Iterator.splitLast. Plus a demo showing splitFirst as a sequential iteration pattern.

---

## 1. Fold — composed from loop + splitFirst

`fold(init, body)`: `Iterator<T> → TAcc` where `body: [TAcc, T] → TAcc`.

Threads an accumulator through elements one at a time, returns the final accumulator. Composed from `loop` + `splitFirst` + `bindInput`. No new AST nodes.

### Implementation

```typescript
// In Iterator namespace:
fold<TElement, TAcc>(
  init: Pipeable<void, TAcc>,
  body: Pipeable<[TAcc, TElement], TAcc>,
): TypedAction<IteratorT<TElement>, TAcc> {
  return Iterator.collect<TElement>()
    .then(bindInput<TElement[]>(elements =>
      all(init, elements)
        .then(loop<TAcc, [TAcc, TElement[]]>((recur, done) =>
          bindInput<[TAcc, TElement[]]>(state => {
            const acc = state.getIndex(0).unwrap();
            const remaining = state.getIndex(1).unwrap();

            return remaining.splitFirst().branch({
              None: acc.then(done),
              Some: bindInput<[TElement, TElement[]]>(headTail => {
                const head = headTail.getIndex(0).unwrap();
                const tail = headTail.getIndex(1).unwrap();

                return all(acc, head)
                  .then(body)
                  .then(bindInput<TAcc>(newAcc =>
                    all(newAcc, tail).then(recur),
                  ));
              }),
            });
          }),
        )),
    ));
},
```

State threaded through loop: `[TAcc, TElement[]]` — accumulator and remaining elements. Each iteration: `splitFirst(remaining)` → `None` means done (return acc), `Some([head, tail])` means run body, recur with new acc and tail. No output collection — just returns the final accumulator.

**Implementation note:** The actual code requires two `typedAction()` re-wrappings to work around TypeScript inference limitations:
1. `done` has type `TypedAction<VoidToNull<TAcc>, never>` — the conditional type can't simplify for generic `TAcc`, so it's re-wrapped as `typedAction<TAcc, never>(toAction(done))`.
2. The loop body's return type infers as `any` due to `bindInput`'s `TOut = any` default cascading through `branch`. The entire body is wrapped with `typedAction<[TAcc, TElement[]], never>(...)` to provide the correct type.

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
        getIndex(1).unwrap().iterate(),                          // → Iterator<T>
      ),
    ));
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
        getIndex(0).unwrap().iterate(),                          // → Iterator<T>
        getIndex(1).unwrap(),                                    // → T
      ),
    ));
},
```

Postfix: same branchFamily pattern as splitFirst.

---

## 4. Demo: sequential iteration with splitFirst

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

**Implementation note:** `loop<null, Iterator<string>>` uses `null` (not `void`) for TBreak because `done` has type `TypedAction<VoidToNull<TBreak>, never>` — `VoidToNull<void>` is `null`, so the None branch must produce `null` via `constant<null>(null).then(done)`. The `bindInput<..., never>` on the Some branch explicitly specifies the output type to prevent `TOut = any` from poisoning branch inference.

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
  type Iterator,
  runPipeline,
  pipe,
  loop,
  drop,
  constant,
  identity,
  bindInput,
} from "@barnum/barnum/pipeline";
import { getServices, deployService, verifyService } from "./handlers/deploy";

console.error("=== Sequential deploy demo ===\n");

runPipeline(
  pipe(
    getServices,
    identity<string[]>().iterate(),

    loop<null, Iterator<string>>((recur, done) =>
      identity<Iterator<string>>()
        .splitFirst()
        .branch({
          None: constant<null>(null).then(done),

          Some: bindInput<[string, Iterator<string>], never>((pair) =>
            pipe(
              pair.getIndex(0).unwrap(),
              deployService,
              verifyService,
              drop,
              pair.getIndex(1).unwrap(),
              recur,
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

## 5. Implementation status

All phases complete.

- **Phase A: Iterator.splitFirst / splitLast** — `libs/barnum/src/iterator.ts` + branchFamily dispatch in `ast.ts`
- **Phase B: Iterator.fold + isEmpty** — `libs/barnum/src/iterator.ts` + postfix methods in `ast.ts`
- **Phase C: Sequential deploy demo** — `demos/sequential-deploy/`
