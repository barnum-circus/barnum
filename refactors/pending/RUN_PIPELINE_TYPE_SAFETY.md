# runPipeline Type Safety

## Bug

`runPipeline` currently accepts any `Action` with an optional `input?` param. This means you can pass a `Pipeable<{artifact: string}, {verified: boolean}>` to `runPipeline()` with no input and get no type error — the engine will run it with undefined input at runtime.

## Fix

Two overloads:

```typescript
// No input: only accepts pipelines that don't need input (void or any)
export function runPipeline<T>(pipeline: Pipeable<void, T>): Promise<T>;

// With input: accepts any pipeline, input must match
export function runPipeline<TIn, TOut>(pipeline: Pipeable<TIn, TOut>, input: TIn): Promise<TOut>;
```

This makes it a compile error to call `runPipeline(verify)` without providing `{ artifact: string }`.
