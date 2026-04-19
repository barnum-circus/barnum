import {
  type Option,
  type TaggedUnion,
  type TypedAction,
  toAction,
  typedAction,
} from "../ast.js";
import { chain } from "../chain.js";
import { all } from "../all.js";
import { constant } from "./scalar.js";
import { wrapInField, merge } from "./struct.js";
import { z } from "zod";

// ---------------------------------------------------------------------------
// Tag — wrap input as a tagged union variant
// ---------------------------------------------------------------------------

/**
 * Wrap input as a tagged union member. Requires the full variant map TDef
 * so the output type carries __def for branch decomposition.
 *
 * Usage: tag<{ Ok: string; Err: number }, "Ok">("Ok")
 *        input: string → output: TaggedUnion<{ Ok: string; Err: number }>
 */
export function tag<
  TEnumName extends string,
  TDef extends Record<string, unknown>,
  TKind extends keyof TDef & string,
>(kind: TKind, enumName: TEnumName): TypedAction<TDef[TKind], TaggedUnion<TEnumName, TDef>> {
  const namespacedKind = `${enumName}.${kind}`;
  return chain(
    toAction(all(
      chain(toAction(constant(namespacedKind)), toAction(wrapInField("kind"))),
      wrapInField("value"),
    )),
    toAction(merge()),
  ) as TypedAction<TDef[TKind], TaggedUnion<TEnumName, TDef>>;
}

// ---------------------------------------------------------------------------
// ExtractPrefix — extract enum prefix from tagged value kind
// ---------------------------------------------------------------------------

/**
 * Extract the enum prefix from a tagged value's `kind` field.
 *
 * Input:  `{ kind: "Result.Ok", value: 42 }`
 * Output: `{ kind: "Result", value: { kind: "Result.Ok", value: 42 } }`
 *
 * If `kind` contains no `'.'`, the entire kind string becomes the prefix.
 * Used internally by `branchFamily` for two-level dispatch.
 */
export function extractPrefix(): TypedAction {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "ExtractPrefix" } },
  });
}

// ---------------------------------------------------------------------------
// AsOption — convert boolean to Option<void>
// ---------------------------------------------------------------------------

/**
 * Convert a boolean to `Option<void>`.
 *
 * `true`  → `{ kind: "Option.Some", value: null }`
 * `false` → `{ kind: "Option.None", value: null }`
 */
export function asOption(): TypedAction<boolean, Option<void>> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "AsOption" } },
  });
}

// ---------------------------------------------------------------------------
// TaggedUnion Zod schema constructor
// ---------------------------------------------------------------------------

/**
 * Reverse of VoidToNull: maps `null` back to `void` in the def so that
 * `taggedUnionSchema({ Clean: z.null() })` produces the same phantom __def
 * as `TaggedUnion<{ Clean: void }>`.
 */
type NullToVoid<TDef> = {
  [K in keyof TDef]: TDef[K] extends null ? void : TDef[K];
};

/**
 * Build a Zod schema for a `TaggedUnion<TEnumName, TDef>` — a discriminated
 * union of `{ kind: "EnumName.Variant"; value: V }` objects.
 *
 * Each key in `cases` becomes a variant with a namespaced kind string.
 * Use `z.null()` for void variants.
 *
 * ```ts
 * const schema = taggedUnionSchema("ClassifyResult", {
 *   HasErrors: z.array(TypeErrorValidator),
 *   Clean: z.null(),
 * });
 * ```
 */
export function taggedUnionSchema<
  TEnumName extends string,
  TDef extends Record<string, z.ZodTypeAny>,
>(
  enumName: TEnumName,
  cases: TDef,
): z.ZodType<
  TaggedUnion<TEnumName, NullToVoid<{ [K in keyof TDef & string]: z.infer<TDef[K]> }>>
> {
  type Out = TaggedUnion<
    TEnumName, NullToVoid<{ [K in keyof TDef & string]: z.infer<TDef[K]> }>
  >;
  const variants = Object.entries(cases).map(([kind, valueSchema]) =>
    z.object({ kind: z.literal(`${enumName}.${kind}`), value: valueSchema }),
  );
  if (variants.length === 0) {
    return z.never() as z.ZodType<Out>;
  }
  if (variants.length === 1) {
    return variants[0] as z.ZodType<Out>;
  }
  return z.discriminatedUnion("kind", [
    variants[0],
    variants[1],
    ...variants.slice(2),
  ]) as z.ZodType<Out>;
}
