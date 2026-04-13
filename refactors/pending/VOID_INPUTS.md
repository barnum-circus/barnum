# Void Inputs for Pipeline-Ignoring Actions

## Status: Pending (future refinement)

## Motivation

Actions that ignore their pipeline input (VarRef, Option.none, constant, drop, range, sleep) currently use `any` as their input type. This works but is imprecise — `any` means "accepts anything" which is true but doesn't communicate that the input is **discarded**.

The preference: use `void` input instead of `any` for these actions. This would require callers to explicitly `.drop()` before piping into them, making it clear in the code that the previous value is being discarded.

## Current state

- `VarRef<T> = TypedAction<any, T>` — ignores pipeline input, reads from bound state
- `Option.none<T>(): TypedAction<any, OptionT<T>>` — ignores input, produces None
- `constant<T>(value: T): TypedAction<any, T>` — ignores input
- `drop: TypedAction<any, never>` — ignores input (discards it)
- `range(start, end): TypedAction<any, number[]>` — ignores input
- `sleep(ms): TypedAction<any, never>` — ignores input

## Proposed change

Change `any` input to `void` input on actions that discard their pipeline input. This forces callers to `.drop()` before piping into them.

```ts
// Before
constant<T>(value: T): TypedAction<any, T>

// After
constant<T>(value: T): TypedAction<void, T>

// Usage
pipe(someAction.drop(), constant(42))  // explicit discard
```

## Dependency

Requires the void vs never change to land first (drop/sleep/.drop() output `void`), so that `.drop()` produces the right type to chain into `void`-input actions.

## Trade-off

- **Pro:** Makes discarded values explicit at call sites. Pipeline reads more honestly.
- **Con:** More verbose — every use of constant/VarRef/etc. after a value-producing step needs `.drop()`.

This is a refinement, not urgent. The `any` input works correctly today.
