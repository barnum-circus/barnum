# Revert Callback Destructuring

## Motivation

Commits `a002b1b1` (withResource) and `1102ff74` (fold) changed these methods to accept callback parameters with VarRef destructuring. The new direction is: methods accept Pipeable, users opt into VarRef/destructuring via `bindInput` + `.split()`.

The principle: combinators accept Pipeable. `bindInput` is the user-facing opt-in when you need VarRefs. `.split()` is how you destructure a VarRef into its components.

## Current State

### withResource (`src/builtins/with-resource.ts`)

```typescript
export function withResource<TIn, TResource, TOut>({
  create,
  action,
  dispose,
}: {
  create: Pipeable<TIn, TResource>;
  action: (resource: VarRef<TResource>, input: VarRef<TIn>) => BodyResult<TOut>;
  dispose: Pipeable<TResource, any>;
}): TypedAction<TIn, TOut> {
  return bindInput<TIn, TOut>((inputRef) =>
    typedAction<TIn, TResource>(create).bindInput<TOut>((resourceRef) =>
      typedAction<void, TOut>(action(resourceRef, inputRef)).bindInput<TOut>(
        (outputRef) =>
          typedAction<TResource, any>(dispose).drop().then(outputRef),
      ),
    ),
  );
}
```

### Iterator.fold (`src/iterator.ts:175-215`)

```typescript
fold<TElement, TAcc>(
  init: Pipeable<void, TAcc>,
  body: (acc: VarRef<TAcc>, element: VarRef<TElement>) => BodyResult<TAcc>,
): TypedAction<IteratorT<TElement>, TAcc> {
  // ... complex implementation using bindInput internally
}
```

## Proposed Changes

### Task 1: Add `.split()` to TypedAction

**File:** `src/ast.ts`

**Goal:** Add a `.split()` method that returns a Proxy supporting both tuple and object destructuring.

#### 1.1: Add `Split<T>` type

```typescript
// After the VarRef type (in bind.ts or ast.ts, wherever VarRef is re-exported)
/**
 * Maps each field/index of T to a VarRef of that field's type.
 * For tuples: Split<[A, B]> = [VarRef<A>, VarRef<B>]
 * For objects: Split<{a: A, b: B}> = {a: VarRef<A>, b: VarRef<B>}
 */
export type Split<T> = { [K in keyof T]: VarRef<T[K]> };
```

#### 1.2: Add `splitMethod` implementation

```typescript
function splitMethod(this: TypedAction): unknown {
  const self = this;
  return new Proxy(
    {},
    {
      get(_, key) {
        if (key === Symbol.iterator) {
          return function* () {
            let i = 0;
            while (true) {
              yield typedAction({
                kind: "Chain",
                first: self,
                rest: toAction(getIndex(i).unwrap()),
              });
              i++;
            }
          };
        }
        if (typeof key === "string") {
          return typedAction({
            kind: "Chain",
            first: self,
            rest: toAction(getField(key)),
          });
        }
        return undefined;
      },
    },
  );
}
```

**How it works:**
- Tuple: `const [a, b] = state.split()` — JS calls `Symbol.iterator`, generator yields `state.getIndex(0).unwrap()`, `state.getIndex(1).unwrap()`. Destructuring takes 2 and stops.
- Object: `const { name, age } = state.split()` — JS calls `get("name")`, `get("age")`. Proxy returns `state.getField("name")`, `state.getField("age")`.

Each yielded/returned value is a full TypedAction (VarRef) — it chains `getIndex(n).unwrap()` or `getField(key)` after `this`.

#### 1.3: Add to TypedAction type definition

```typescript
// In the TypedAction type, alongside other methods:
/** Destructure into component VarRefs. Supports tuple and object patterns. */
split(): Split<Out>;
```

#### 1.4: Register in `typedAction()` factory

```typescript
// In the Object.defineProperties block inside typedAction():
split: { value: splitMethod, writable: false, enumerable: false, configurable: false },
```

### Task 2: Revert withResource

**File:** `src/builtins/with-resource.ts`

**Before (current):**
```typescript
export function withResource<TIn, TResource, TOut>({
  create,
  action,
  dispose,
}: {
  create: Pipeable<TIn, TResource>;
  action: (resource: VarRef<TResource>, input: VarRef<TIn>) => BodyResult<TOut>;
  dispose: Pipeable<TResource, any>;
}): TypedAction<TIn, TOut> {
  return bindInput<TIn, TOut>((inputRef) =>
    typedAction<TIn, TResource>(create).bindInput<TOut>((resourceRef) =>
      typedAction<void, TOut>(action(resourceRef, inputRef)).bindInput<TOut>(
        (outputRef) =>
          typedAction<TResource, any>(dispose).drop().then(outputRef),
      ),
    ),
  );
}
```

**After:**
```typescript
export function withResource<TIn, TResource, TOut>({
  create,
  action,
  dispose,
}: {
  create: Pipeable<TIn, TResource>;
  action: Pipeable<[TResource, TIn], TOut>;
  dispose: Pipeable<TResource, any>;
}): TypedAction<TIn, TOut> {
  return bindInput<TIn, TOut>((inputRef) =>
    inputRef.then(create).bindInput<TOut>((resourceRef) =>
      all(resourceRef, inputRef)
        .then(action)
        .bindInput<TOut>((outputRef) =>
          resourceRef.then(dispose).drop().then(outputRef),
        ),
    ),
  );
}
```

**User-facing usage:**
```typescript
withResource({
  create: createDb,
  action: bindInput<[Db, Config]>((state) => {
    const [db, config] = state.split();
    return db.then(query(config));
  }),
  dispose: closeDb,
})
```

### Task 3: Revert Iterator.fold

**File:** `src/iterator.ts`

**Before (current):**
```typescript
fold<TElement, TAcc>(
  init: Pipeable<void, TAcc>,
  body: (acc: VarRef<TAcc>, element: VarRef<TElement>) => BodyResult<TAcc>,
): TypedAction<IteratorT<TElement>, TAcc>
```

**After:**
```typescript
fold<TElement, TAcc>(
  init: Pipeable<void, TAcc>,
  body: Pipeable<[TAcc, TElement], TAcc>,
): TypedAction<IteratorT<TElement>, TAcc>
```

The internal implementation of fold already uses `bindInput` internally to manage the loop state. The change is only to the `body` parameter signature — it goes from being a callback to being a Pipeable.

**User-facing usage:**
```typescript
Iterator.fold(
  constant(0),
  bindInput<[number, Item]>((state) => {
    const [acc, item] = state.split();
    return item.then(getScore).then(add(acc));
  }),
)
```

### Task 4: Update tests

Update `tests/with-resource.test.ts` and `tests/iterator.test.ts` to use the new Pipeable-based signatures with `bindInput` + `.split()` at callsites.

## Implementation Order

1. Add `.split()` to TypedAction (Task 1)
2. Revert `withResource` (Task 2)
3. Revert `fold` (Task 3)
4. Update tests (Task 4)
5. Typecheck + test suite passes
