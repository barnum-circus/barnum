import {
  type Action,
  type MergeTuple,
  type TypedAction,
  toAction,
  typedAction,
} from "../ast.js";
import { chain } from "../chain.js";

// ---------------------------------------------------------------------------
// GetField — extract a single field from an object
// ---------------------------------------------------------------------------

export function getField<
  TObj extends Record<string, unknown>,
  TField extends keyof TObj & string,
>(field: TField): TypedAction<TObj, TObj[TField]> {
  return typedAction({
    kind: "Invoke",
    handler: {
      kind: "Builtin",
      builtin: { kind: "GetField", field },
    },
  });
}

// ---------------------------------------------------------------------------
// WrapInField — wrap input as { <field>: <input> }
// ---------------------------------------------------------------------------

export function wrapInField<TField extends string, TValue>(
  field: TField,
): TypedAction<TValue, Record<TField, TValue>> {
  return typedAction({
    kind: "Invoke",
    handler: {
      kind: "Builtin",
      builtin: { kind: "WrapInField", field },
    },
  });
}

// ---------------------------------------------------------------------------
// Merge — merge a tuple of objects into a single object
// ---------------------------------------------------------------------------

export function merge<TTuple extends Record<string, unknown>[]>(): TypedAction<
  TTuple,
  MergeTuple<TTuple>
> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Merge" } },
  });
}

// ---------------------------------------------------------------------------
// Pick — select named fields from an object
// ---------------------------------------------------------------------------

export function pick<
  TObj extends Record<string, unknown>,
  TKeys extends (keyof TObj & string)[],
>(...keys: TKeys): TypedAction<TObj, Pick<TObj, TKeys[number]>> {
  const actions = keys.map(
    (key) => toAction(chain(toAction(getField(key)), toAction(wrapInField(key)))),
  );
  const allAction: Action = { kind: "All", actions };
  return chain(toAction(allAction), toAction(merge())) as TypedAction<
    TObj,
    Pick<TObj, TKeys[number]>
  >;
}
