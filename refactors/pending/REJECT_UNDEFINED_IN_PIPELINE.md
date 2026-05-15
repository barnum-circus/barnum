# Reject `undefined` in Pipeline Values at the Type Level

## Motivation

`undefined` has no JSON representation. `JSON.stringify({ value: undefined })` produces `{}`, causing the Rust deserializer to fail with "missing field." TypeScript doesn't catch this because `void` accepts `undefined` and `constant(undefined)` type-checks fine.

This is a gap between TypeScript's type system and the JSON serialization boundary. Users hit it when writing branch cases like `constant(undefined)` for "no-op" paths.

## Current state

`constant<TValue>(value: TValue)` accepts any value including `undefined`. The `createHandlerWithConfig` factory passes `unknown` config into `constant(config)` at `libs/barnum/src/handler.ts:251`.

## Why it's not a one-line fix

A constraint like `TValue extends {} | null` or a conditional `undefined extends TValue ? never : TValue` rejects `unknown` too — and `createHandlerWithConfig` passes `unknown` because the config type is erased at that point. Fixing `constant` requires also fixing the handler factory to thread a concrete config type all the way through.

## Proposed approach

1. **Constrain `constant`** to reject `undefined`:
   ```typescript
   export function constant<TValue extends {} | null>(value: TValue): TypedAction<any, TValue>
   ```

2. **Fix `createHandlerWithConfig`** to preserve the config type through the factory. The factory function currently types config as `unknown` — it should carry the generic `TConfig` all the way to the `constant()` call:
   ```typescript
   const factory = <TConfig extends {} | null>(config: TConfig): TypedAction =>
     chain(toAction(all(identity(), constant(config))), toAction(invokeAction));
   ```

3. **Audit other internal `constant()` call sites** for similar issues.

## Scope

Small. The handler factory is the only internal caller that passes an unconstrained type to `constant`. The fix is threading the generic through one function.
