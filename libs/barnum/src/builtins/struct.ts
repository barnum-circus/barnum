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

export function merge<
  TTuple extends Array<Record<string, unknown>>,
>(): TypedAction<TTuple, MergeTuple<TTuple>> {
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
  TKeys extends Array<keyof TObj & string>,
>(...keys: TKeys): TypedAction<TObj, Pick<TObj, TKeys[number]>> {
  const actions: Array<Action> = keys.map((key) =>
    chain(getField(key), wrapInField(key)),
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

type InferIn<T> = T extends Pipeable<infer I, any> ? I : never;

// Union of all action inputs across the record.
// Because Pipeable is invariant, this doubles as the "required input" each
// action must have: any action whose input is not exactly this union will fail.
type AllInputs<T extends Record<string, Pipeable<any, any>>> = InferIn<
  T[keyof T]
>;

// Detect the degenerate empty-record case.
type IsNever<T> = [T] extends [never] ? true : false;

// Per-action validation. Each slot must have input exactly equal to AllInputs<T>.
// Invariance of Pipeable means only exact-match passes; narrower or wider both fail.
type ValidateActions<T extends Record<string, Pipeable<any, any>>> = {
  [K in keyof T]: T[K] extends Pipeable<AllInputs<T>, any>
    ? T[K]
    : Pipeable<AllInputs<T>, any>;
};

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
export function allObject<
  const TActions extends Record<string, Pipeable<any, any>>,
>(
  actions: IsNever<AllInputs<TActions>> extends true
    ? { __error: "allObject requires at least one action"; actions: TActions }
    : TActions & ValidateActions<TActions>,
): TypedAction<
  AllInputs<TActions>,
  { [K in keyof TActions & string]: ExtractOutput<TActions[K]> }
> {
  const wrapped: Array<Action> = Object.entries(actions).map(([key, action]) =>
    chain(action, wrapInField(key)),
  );
  const allAction: Action = { kind: "All", actions: wrapped };
  return chain(toAction(allAction), toAction(merge())) as TypedAction<
    any,
    { [K in keyof TActions & string]: ExtractOutput<TActions[K]> }
  >;
}
