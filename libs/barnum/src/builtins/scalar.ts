import { type TypedAction, typedAction } from "../ast.js";

// ---------------------------------------------------------------------------
// Constant — produce a fixed value (takes no pipeline input)
// ---------------------------------------------------------------------------

export function constant<TValue>(value: TValue): TypedAction<any, TValue> {
  return typedAction({
    kind: "Invoke",
    handler: {
      kind: "Builtin",
      builtin: { kind: "Constant", value: value ?? null },
    },
  });
}

// ---------------------------------------------------------------------------
// Identity — pass input through unchanged
// ---------------------------------------------------------------------------

export function identity<TValue = any>(): TypedAction<TValue, TValue> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Identity" } },
  });
}

// ---------------------------------------------------------------------------
// Drop — discard pipeline value
// ---------------------------------------------------------------------------

export const drop: TypedAction<any, null> = typedAction({
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "Drop" } },
});

// ---------------------------------------------------------------------------
// Panic — halt execution with an error message
// ---------------------------------------------------------------------------

/**
 * Halt execution with a fatal error. Not caught by tryCatch.
 * Analogous to Rust's `panic!`.
 *
 * The input value is included in the error message as JSON.
 * Pass `null` (via `drop`) when the input isn't relevant.
 *
 * Output type is `never` — a panic never produces a value.
 */
export function panic(message: string): TypedAction<any, never> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Panic", message } },
  });
}
