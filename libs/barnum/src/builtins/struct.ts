import {
  type Action,
  type ExtractOutput,
  type MergeTuple,
  type Pipeable,
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
  const actions = keys.map((key) =>
    toAction(chain(toAction(getField(key)), toAction(wrapInField(key)))),
  );
  const allAction: Action = { kind: "All", actions };
  return chain(toAction(allAction), toAction(merge())) as TypedAction<
    TObj,
    Pick<TObj, TKeys[number]>
  >;
}

// ---------------------------------------------------------------------------
// AllObject — run named actions concurrently, collect into an object
// ---------------------------------------------------------------------------

/**
 * Run named actions concurrently on the same input, collecting results
 * into an object with matching keys.
 *
 * ```ts
 * allObject({
 *   files: listFiles,
 *   config: loadConfig,
 * })
 * // TIn → { files: string[], config: Config }
 * ```
 *
 * Each action receives the pipeline input. Results are wrapped in
 * `{ key: value }` via `wrapInField`, run concurrently via `All`,
 * then merged into a single object.
 */
export function allObject<TActions extends Record<string, Pipeable<any, any>>>(
  actions: TActions,
): TypedAction<
  any,
  { [K in keyof TActions & string]: ExtractOutput<TActions[K]> }
> {
  const wrapped = Object.entries(actions).map(([key, action]) =>
    toAction(chain(action, wrapInField(key))),
  );
  const allAction: Action = { kind: "All", actions: wrapped };
  return chain(toAction(allAction), toAction(merge())) as TypedAction<
    any,
    { [K in keyof TActions & string]: ExtractOutput<TActions[K]> }
  >;
}
