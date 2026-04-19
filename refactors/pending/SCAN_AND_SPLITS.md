# Scan and Splits — Priority Iterator Implementations

Concrete implementations for the next batch of Iterator methods: scan (new AST node), take/skip (new builtins), splitFirst/splitLast on Iterator (composable), and derived terminals.

---

## 1. Scan — new AST node

The fundamental sequential primitive. Every element depends on the previous accumulator, so this cannot be expressed as `ForEach` (which is parallel). Scan unlocks fold, reduce, and forEachSync.

### 1.1 AST definition (TypeScript)

New variant in the `Action` union in `ast.ts`:

```typescript
export type Action =
  | InvokeAction
  | ChainAction
  | ForEachAction
  | AllAction
  | BranchAction
  | ScanAction          // NEW
  | ResumeHandleAction
  | ResumePerformAction
  | RestartHandleAction
  | RestartPerformAction;

export interface ScanAction {
  kind: "Scan";
  init: Action;   // void → TAcc (evaluated once, produces initial accumulator)
  body: Action;   // [TAcc, TElement] → [TAcc, TOutput] (per-element transform)
}
```

### 1.2 AST definition (Rust)

New variant in the `Action` enum in `barnum_ast`:

```rust
// In the Action enum:
Scan(ScanAction),

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScanAction {
    /// Produces the initial accumulator value. Evaluated with null input.
    pub init: Box<Action>,
    /// Per-element body. Receives [acc, element] as input, returns [newAcc, output].
    pub body: Box<Action>,
}
```

### 1.3 Engine execution

The engine receives the Scan node. The pipeline input is `T[]` (the source array).

```
1. Evaluate `init` with null input → acc₀
2. For each element_i in the source array:
   a. Construct input: [acc_i, element_i]
   b. Evaluate `body` with this input → result
   c. acc_{i+1} = result[0]  (new accumulator)
   d. output_i  = result[1]  (emitted value)
3. Return [output_0, output_1, ..., output_{n-1}]
```

Sequential — each iteration depends on the previous accumulator. The Rust scheduler must process elements one at a time, not fan out like ForEach.

Empty input array: scan produces an empty output array. The init action is still evaluated (side effects run), but body never executes.

### 1.4 TypeScript constructor

```typescript
// In ast.ts (alongside forEach):
export function scan<TElement, TAcc, TOut>(
  init: Pipeable<void, TAcc>,
  body: Pipeable<[TAcc, TElement], [TAcc, TOut]>,
): TypedAction<TElement[], TOut[]> {
  return typedAction({ kind: "Scan", init: toAction(init), body: toAction(body) });
}
```

### 1.5 Iterator.scan

```typescript
// In Iterator namespace in iterator.ts:
scan<TElement, TAcc, TOut>(
  init: Pipeable<void, TAcc>,
  body: Pipeable<[TAcc, TElement], [TAcc, TOut]>,
): TypedAction<IteratorT<TElement>, IteratorT<TOut>> {
  return chain(
    toAction(getField("value")),
    chain(toAction(scan(init, body)), Iterator.fromArray<TOut>()),
  ) as TypedAction<IteratorT<TElement>, IteratorT<TOut>>;
},
```

Pattern: unwrap Iterator → run scan on raw array → re-wrap as Iterator. Same shape as `Iterator.map`.

### 1.6 Iterator.fold (composable from scan)

`fold(init, f)` reduces an Iterator to a single value. `f: [TAcc, TElement] → TAcc`. Returns the final accumulator, or `init` if the Iterator is empty.

```typescript
// In Iterator namespace:
fold<TElement, TAcc>(
  init: Pipeable<void, TAcc>,
  body: Pipeable<[TAcc, TElement], TAcc>,
): TypedAction<IteratorT<TElement>, TAcc> {
  // Wrap body to duplicate output: [acc, elem] → newAcc → [newAcc, newAcc]
  // all(identity(), identity()) turns TAcc into [TAcc, TAcc]
  const scanBody = chain(body, all(identity(), identity()));

  // scan(init, wrappedBody) → TAcc[] (all intermediate accumulators)
  // last() → Option<TAcc>
  // unwrapOr(init) → TAcc (handles empty iterator)
  return chain(
    toAction(getField("value")),
    chain(
      toAction(scan(init, scanBody)),
      chain(toAction(last()), toAction(Option.unwrapOr(init))),
    ),
  ) as TypedAction<IteratorT<TElement>, TAcc>;
},
```

The trick: scan emits the new accumulator as both `newAcc` and `output`. After scan runs, we take the last output (which is the final accumulator). If the iterator was empty, scan produces `[]`, `last()` returns None, and `unwrapOr(init)` falls back to the initial value.

### 1.7 Iterator.reduce (composable from scan)

`reduce(f)` folds without an initial value. Uses the first element as the initial accumulator. Returns `Option<T>` (None for empty iterators).

```typescript
// In Iterator namespace:
reduce<TElement>(
  body: Pipeable<[TElement, TElement], TElement>,
): TypedAction<IteratorT<TElement>, OptionT<TElement>> {
  const scanBody = chain(body, all(identity(), identity()));

  return chain(
    Iterator.collect<TElement>(),
    chain(
      toAction(splitFirst()),
      // Option<[TElement, TElement[]]> → Option<TElement>
      toAction(Option.map(
        bindInput<[TElement, TElement[]]>(pair => {
          const firstElem = pair.getIndex(0).unwrap();   // VarRef<TElement>
          const rest = pair.getIndex(1).unwrap();         // VarRef<TElement[]>
          // fold rest using firstElem as init
          // firstElem is a VarRef — evaluated via ResumeHandle, ignores scan's null input
          return chain(
            toAction(rest),
            chain(
              toAction(scan(firstElem, scanBody)),
              chain(toAction(last()), toAction(Option.unwrapOr(firstElem))),
            ),
          );
        }),
      )),
    ),
  ) as TypedAction<IteratorT<TElement>, OptionT<TElement>>;
},
```

How it works:
1. `collect()` → `T[]`
2. `splitFirst()` → `Option<[T, T[]]>`
3. If None (empty): result is None
4. If Some([first, rest]): `scan(first, body)` over rest, take last, fall back to first if rest is empty

The VarRef `firstElem` works as scan's init because VarRefs are evaluated via the ResumeHandle mechanism — they return the bound value regardless of the current pipeline input. So even though scan evaluates init with null, the VarRef produces the captured first element.

### 1.8 Postfix wiring

In `ast.ts`, add to the `TypedAction` type:

```typescript
// Iterator postfix methods:
scan<TIn, TElement, TAcc, TOut>(
  this: TypedAction<TIn, Iterator<TElement>>,
  init: Pipeable<void, TAcc>,
  body: Pipeable<[TAcc, TElement], [TAcc, TOut]>,
): TypedAction<TIn, Iterator<TOut>>;

fold<TIn, TElement, TAcc>(
  this: TypedAction<TIn, Iterator<TElement>>,
  init: Pipeable<void, TAcc>,
  body: Pipeable<[TAcc, TElement], TAcc>,
): TypedAction<TIn, TAcc>;

reduce<TIn, TElement>(
  this: TypedAction<TIn, Iterator<TElement>>,
  body: Pipeable<[TElement, TElement], TElement>,
): TypedAction<TIn, Option<TElement>>;
```

In `typedAction()`, add method implementations:

```typescript
function scanMethod(this: TypedAction, init: Action, body: Action): TypedAction {
  return chain(toAction(this), toAction(IteratorNs.scan(init, body)));
}

function foldMethod(this: TypedAction, init: Action, body: Action): TypedAction {
  return chain(toAction(this), toAction(IteratorNs.fold(init, body)));
}

function reduceMethod(this: TypedAction, body: Action): TypedAction {
  return chain(toAction(this), toAction(IteratorNs.reduce(body)));
}

// In Object.defineProperties:
scan: { value: scanMethod, configurable: true },
fold: { value: foldMethod, configurable: true },
reduce: { value: reduceMethod, configurable: true },
```

---

## 2. Take / Skip — new builtins

Array slicing primitives. `take(n)` returns the first n elements, `skip(n)` drops the first n.

### 2.1 Builtin definitions

In `BuiltinKind` in `ast.ts`:

```typescript
| { kind: "Take"; n: number }
| { kind: "Skip"; n: number }
```

In Rust `BuiltinKind` enum:

```rust
Take { n: usize },
Skip { n: usize },
```

Rust execution:
- `Take { n }`: `input.into_iter().take(n).collect()` (or `input[..n.min(input.len())]`)
- `Skip { n }`: `input.into_iter().skip(n).collect()` (or `input[n.min(input.len())..]`)

Both clamp to array length — `take(100)` on a 3-element array returns 3 elements, `skip(100)` returns `[]`.

### 2.2 TypeScript constructors

In `builtins/array.ts`:

```typescript
export function take<TElement>(n: number): TypedAction<TElement[], TElement[]> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Take", n } },
  });
}

export function skip<TElement>(n: number): TypedAction<TElement[], TElement[]> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Skip", n } },
  });
}
```

### 2.3 Iterator.take / Iterator.skip

```typescript
// In Iterator namespace:
take<TElement>(n: number): TypedAction<IteratorT<TElement>, IteratorT<TElement>> {
  return chain(
    toAction(getField("value")),
    chain(toAction(take(n)), Iterator.fromArray<TElement>()),
  ) as TypedAction<IteratorT<TElement>, IteratorT<TElement>>;
},

skip<TElement>(n: number): TypedAction<IteratorT<TElement>, IteratorT<TElement>> {
  return chain(
    toAction(getField("value")),
    chain(toAction(skip(n)), Iterator.fromArray<TElement>()),
  ) as TypedAction<IteratorT<TElement>, IteratorT<TElement>>;
},
```

### 2.4 splitFirstN (composable)

`splitFirstN(n)` splits at position n, returning `[first_n, rest]`. Composable from take + skip:

```typescript
// Standalone — not in Iterator namespace:
export function splitFirstN<TElement>(
  n: number,
): TypedAction<TElement[], [TElement[], TElement[]]> {
  return all(take(n), skip(n)) as TypedAction<TElement[], [TElement[], TElement[]]>;
}
```

`all(take(n), skip(n))` runs both on the same input and returns the pair. One conceptual pass (two actual slices on the same backing array — cheap).

### 2.5 Postfix wiring

```typescript
// In TypedAction type:
take<TIn, TElement>(
  this: TypedAction<TIn, Iterator<TElement>>,
  n: number,
): TypedAction<TIn, Iterator<TElement>>;

skip<TIn, TElement>(
  this: TypedAction<TIn, Iterator<TElement>>,
  n: number,
): TypedAction<TIn, Iterator<TElement>>;

// In typedAction():
function takeMethod(this: TypedAction, n: number): TypedAction {
  return chain(toAction(this), toAction(IteratorNs.take(n)));
}
function skipMethod(this: TypedAction, n: number): TypedAction {
  return chain(toAction(this), toAction(IteratorNs.skip(n)));
}
// In Object.defineProperties:
take: { value: takeMethod, configurable: true },
skip: { value: skipMethod, configurable: true },
```

---

## 3. Iterator.splitFirst / Iterator.splitLast — composable

These compose from existing `SplitFirst` and `SplitLast` builtins. The Iterator versions unwrap the Iterator, call the array builtin, then re-wrap the remainder as Iterator.

### 3.1 Iterator.splitFirst

`Iterator<T> → Option<[T, Iterator<T>]>`

```typescript
// In Iterator namespace:
splitFirst<TElement>(): TypedAction<
  IteratorT<TElement>,
  OptionT<[TElement, IteratorT<TElement>]>
> {
  return chain(
    toAction(getField("value")),             // Iterator<T> → T[]
    chain(
      toAction(splitFirst()),                // T[] → Option<[T, T[]]>
      toAction(Option.map(
        // [T, T[]] → [T, Iterator<T>]
        all(
          toAction(getIndex(0).unwrap()),                               // → T
          chain(toAction(getIndex(1).unwrap()), Iterator.fromArray()),  // → Iterator<T>
        ),
      )),
    ),
  ) as TypedAction<IteratorT<TElement>, OptionT<[TElement, IteratorT<TElement>]>>;
},
```

How it works:
1. `getField("value")`: unwrap Iterator to `T[]`
2. `splitFirst()`: `T[] → Option<[T, T[]]>` (SplitFirst builtin)
3. `Option.map(...)`: if Some, transform `[T, T[]]` into `[T, Iterator<T>]`
   - `getIndex(0).unwrap()`: extract first element
   - `chain(getIndex(1).unwrap(), fromArray())`: extract rest array, re-wrap as Iterator
   - `all(...)`: pair them back together

### 3.2 Iterator.splitLast

`Iterator<T> → Option<[Iterator<T>, T]>`

```typescript
// In Iterator namespace:
splitLast<TElement>(): TypedAction<
  IteratorT<TElement>,
  OptionT<[IteratorT<TElement>, TElement]>
> {
  return chain(
    toAction(getField("value")),             // Iterator<T> → T[]
    chain(
      toAction(splitLast()),                 // T[] → Option<[T[], T]>
      toAction(Option.map(
        // [T[], T] → [Iterator<T>, T]
        all(
          chain(toAction(getIndex(0).unwrap()), Iterator.fromArray()),  // → Iterator<T>
          toAction(getIndex(1).unwrap()),                               // → T
        ),
      )),
    ),
  ) as TypedAction<IteratorT<TElement>, OptionT<[IteratorT<TElement>, TElement]>>;
},
```

Mirror of splitFirst — `getIndex(0)` is the init array (re-wrapped as Iterator), `getIndex(1)` is the last element.

### 3.3 Iterator.first / Iterator.last (derived)

These fall out trivially — splitFirst/splitLast then extract just the element.

```typescript
// In Iterator namespace:
first<TElement>(): TypedAction<IteratorT<TElement>, OptionT<TElement>> {
  return chain(
    toAction(getField("value")),
    chain(
      toAction(splitFirst()),
      toAction(Option.map(toAction(getIndex(0).unwrap()))),
    ),
  ) as TypedAction<IteratorT<TElement>, OptionT<TElement>>;
},

last<TElement>(): TypedAction<IteratorT<TElement>, OptionT<TElement>> {
  return chain(
    toAction(getField("value")),
    chain(
      toAction(splitLast()),
      toAction(Option.map(toAction(getIndex(1).unwrap()))),
    ),
  ) as TypedAction<IteratorT<TElement>, OptionT<TElement>>;
},
```

### 3.4 Postfix wiring

```typescript
// In TypedAction type:
splitFirst<TIn, TElement>(
  this: TypedAction<TIn, Iterator<TElement>>,
): TypedAction<TIn, Option<[TElement, Iterator<TElement>]>>;

splitLast<TIn, TElement>(
  this: TypedAction<TIn, Iterator<TElement>>,
): TypedAction<TIn, Option<[Iterator<TElement>, TElement]>>;

first<TIn, TElement>(
  this: TypedAction<TIn, Iterator<TElement>>,
): TypedAction<TIn, Option<TElement>>;

last<TIn, TElement>(
  this: TypedAction<TIn, Iterator<TElement>>,
): TypedAction<TIn, Option<TElement>>;
```

Note: `splitFirst` and `splitLast` already exist as postfix methods on arrays (`T[] → Option<[T, T[]]>`). The Iterator overloads are additional signatures. The `typedAction()` method implementations need to dispatch based on whether the output is an Iterator or an array. This follows the same branchFamily pattern as `.map()` and `.collect()`:

```typescript
function splitFirstMethod(this: TypedAction): TypedAction {
  return chain(
    toAction(this),
    toAction(
      branchFamily({
        Array: splitFirst(),                   // existing: T[] → Option<[T, T[]]>
        Iterator: IteratorNs.splitFirst(),     // new: Iterator<T> → Option<[T, Iterator<T>]>
      }),
    ),
  );
}
```

Wait — the existing `splitFirst` postfix doesn't use branchFamily. It's a direct call to the SplitFirst builtin. To add Iterator dispatch, we'd need to change the postfix from direct-call to branchFamily dispatch. Same pattern as how `.map()` dispatches across Option/Result/Iterator.

**Alternative:** don't dispatch. Keep `.splitFirst()` as array-only postfix. Add separate `.iterSplitFirst()` or have users call `Iterator.splitFirst()` as a namespace function and chain it with `.then()`. This avoids the dispatch complexity.

**Recommendation:** use branchFamily dispatch (same as `.map()`). The Iterator is tagged with `kind: "Iterator.Iterator"`, so ExtractPrefix produces `{ kind: "Iterator", value: ... }`. The array case is the fallback. Implementation:

```typescript
function splitFirstMethod(this: TypedAction): TypedAction {
  return chain(
    toAction(this),
    toAction(
      branchFamily({
        Iterator: IteratorNs.splitFirst(),
        Array: splitFirst(),
      }),
    ),
  );
}

function splitLastMethod(this: TypedAction): TypedAction {
  return chain(
    toAction(this),
    toAction(
      branchFamily({
        Iterator: IteratorNs.splitLast(),
        Array: splitLast(),
      }),
    ),
  );
}
```

For `.first()` and `.last()` — these are new postfix methods (no existing array postfix to conflict with):

```typescript
function firstMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(IteratorNs.first()));
}

function lastMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(IteratorNs.last()));
}

// In Object.defineProperties:
first: { value: firstMethod, configurable: true },
last: { value: lastMethod, configurable: true },
```

---

## 4. Demo: sequential iteration with splitFirst

`splitFirst` + `loop` + `branch` is the sequential counterpart to `.iterate().map()`. Where `.map()` dispatches all elements to a handler in parallel via ForEach, the splitFirst loop processes one element at a time in order.

Use splitFirst when:
- Ordering matters (each step depends on the previous)
- You need to accumulate state across elements
- Side effects must happen in sequence (e.g., sequential API calls with rate limiting)

Use `.iterate().map()` when:
- Elements are independent
- Parallel execution is desirable (e.g., concurrent LLM calls)

### Demo: sequential file processing

A workflow that processes files one at a time, accumulating a report. Each file's result may influence how the next file is processed (e.g., tracking cumulative line count).

**`demos/sequential-processing/run.ts`:**

```typescript
import { runPipeline, pipe, loop, constant, identity } from "barnum";

import { listFiles } from "./handlers/files.js";
import { processFile } from "./handlers/process.js";
import { formatReport } from "./handlers/report.js";

// Process files one at a time, accumulating results into a report.
//
// Pattern: loop + splitFirst + branch
//   - splitFirst decomposes [head, ...tail]
//   - process head, append result to accumulator
//   - recur with [newAcc, tail]
//   - when tail is empty, done with accumulated results

const workflow = pipe(
  // listFiles returns string[] (file paths)
  listFiles,

  // Pair with empty accumulator: [ProcessedFile[], string[]]
  identity<string[]>().bind(
    [constant<string[]>([])],
    ([accumulator]) =>
      loop<string, [string[], string[]]>((recur, done) =>
        // Pipeline input: [ProcessedFile[], string[]] — [accumulated results, remaining files]
        identity<[string[], string[]]>()
          .getIndex(1)
          .unwrap()              // extract remaining files: string[]
          .iterate()             // Iterator<string>
          .splitFirst()          // Option<[string, Iterator<string>]>
          .branch({
            // No more files — done, return accumulated results
            None: accumulator.then(done),

            // More files — process head, recur with tail
            Some: identity<[string, Iterator<string>]>()
              .bind(
                [
                  identity<[string, Iterator<string>]>().getIndex(0).unwrap(),   // head: string
                  identity<[string, Iterator<string>]>().getIndex(1).unwrap(),   // tail: Iterator<string>
                ],
                ([head, tail]) =>
                  pipe(
                    head,
                    processFile,
                    // Append result to accumulator, pair with remaining files
                    identity().bind(
                      [tail.collect()],
                      ([remaining]) =>
                        // Build [newAcc, remaining] and recur
                        // newAcc = [...accumulator, thisResult]
                        // TODO: need array append — for now, wrap in array and flatten
                        identity().wrapInField("result").then(recur),  // simplified
                    ),
                  ),
              ),
          }),
      ),
  ),

  formatReport,
);

const result = await runPipeline(workflow);
console.log(result);
```

**This demo is aspirational** — it depends on scan, take/skip, and splitFirst all landing. A simpler demo that works with just splitFirst:

### Minimal demo: ordered pipeline

Process items in strict order. No accumulator, just sequential execution.

**`demos/sequential-processing/run.ts`:**

```typescript
import { runPipeline, pipe, loop, constant } from "barnum";

import { getItems } from "./handlers/items.js";
import { processItem } from "./handlers/process.js";

// Process items one at a time in order.
// splitFirst pattern: loop { splitFirst → Some([head, tail]): process head, recur(tail). None: done. }
const workflow = pipe(
  getItems,                           // → string[]
  identity<string[]>().iterate(),     // → Iterator<string>
  loop<void, Iterator<string>>((recur, done) =>
    Iterator.splitFirst<string>()     // → Option<[string, Iterator<string>]>
      .branch({
        None: done,                   // empty — exit loop
        Some: pipe(
          getIndex(0).unwrap(),       // extract head
          processItem,                // process it (side effects in order)
          drop,                       // discard result
          getIndex(1).unwrap(),       // extract tail (TODO: need bindInput to access both)
          recur,                      // continue with remaining
        ),
      }),
  ),
);

await runPipeline(workflow);
```

Hmm — the `Some` branch has a problem: after `getIndex(0).unwrap()` consumes the pair, we've lost access to `getIndex(1)`. We need `bindInput` to capture the pair:

```typescript
const workflow = pipe(
  getItems,
  identity<string[]>().iterate(),
  loop<void, Iterator<string>>((recur, done) =>
    Iterator.splitFirst<string>()
      .branch({
        None: done,
        Some: bindInput<[string, Iterator<string>]>(pair =>
          pipe(
            pair.getIndex(0).unwrap(),     // head
            processItem,                   // process in order
            drop,                          // discard result
            pair.getIndex(1).unwrap(),     // tail
            recur,                         // continue
          ),
        ),
      }),
  ),
);
```

This is the canonical splitFirst iteration pattern:
1. `splitFirst()` → `Option<[head, tail]>`
2. `None` → done
3. `Some` → `bindInput(pair => process(pair[0]), recur(pair[1]))`

The key insight: `bindInput` captures the `[head, tail]` pair so both components are accessible after the first is consumed by `processItem`.

---

## 5. Implementation order

These can be implemented in phases. Each phase is independently deployable.

### Phase A: Scan AST node (Rust + TypeScript)

1. Add `ScanAction` to Rust `Action` enum
2. Add serde support for Scan
3. Implement sequential execution in the Rust scheduler
4. Add `ScanAction` interface to TypeScript `Action` union
5. Add `scan()` constructor to ast.ts
6. Add `Iterator.scan()` to iterator.ts
7. Test: `[1, 2, 3].iterate().scan(constant(0), addAccBody).collect()` → `[1, 3, 6]`

### Phase B: Take / Skip builtins (Rust + TypeScript)

1. Add `Take` and `Skip` to Rust `BuiltinKind`
2. Implement in Rust (trivial slice operations)
3. Add to TypeScript `BuiltinKind`
4. Add `take()` and `skip()` constructors to `builtins/array.ts`
5. Add `Iterator.take()` and `Iterator.skip()` to iterator.ts
6. Test: `[1,2,3,4,5].iterate().take(3).collect()` → `[1,2,3]`
7. Test: `[1,2,3,4,5].iterate().skip(2).collect()` → `[3,4,5]`

### Phase C: Iterator splits and terminals (TypeScript only)

No new Rust code — all composable from existing builtins.

1. Add `Iterator.splitFirst()` and `Iterator.splitLast()` to iterator.ts
2. Add `Iterator.first()` and `Iterator.last()` to iterator.ts
3. Add `Iterator.fold()` and `Iterator.reduce()` to iterator.ts (depends on Phase A)
4. Wire up postfix methods in ast.ts
5. Update splitFirst/splitLast postfix to use branchFamily dispatch (Iterator + Array)
6. Test all of the above

### Phase D: Sequential processing demo

1. Create `demos/sequential-processing/` with the splitFirst loop pattern
2. Handlers: `getItems` (returns array), `processItem` (processes one item with side effects)
3. Demonstrate ordered execution vs parallel `.iterate().map()`
