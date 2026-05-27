import type { ExtractInput, ExtractOutput } from "../src/ast.js";

// ---------------------------------------------------------------------------
// Type assertion helpers (compile-time only)
// ---------------------------------------------------------------------------

export type IsAny<T> = 0 extends 1 & T ? true : false;

export type IsExact<T, U> =
  IsAny<T> extends IsAny<U>
    ? [T] extends [U]
      ? [U] extends [T]
        ? true
        : false
      : false
    : false;

// eslint-disable-next-line @typescript-eslint/no-unused-vars
export function assertExact<_T extends true>(): void {}

/**
 * Check both input and output types of an action match exactly.
 * Usage: `assertExact<CheckIO<typeof action, ExpectedIn, ExpectedOut>>()`
 */
export type CheckIO<TAction, TIn, TOut> =
  IsExact<ExtractInput<TAction>, TIn> extends true
    ? IsExact<ExtractOutput<TAction>, TOut> extends true
      ? true
      : false
    : false;
