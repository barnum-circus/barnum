import type { JSONSchema7 } from "json-schema";
import { type z, toJSONSchema } from "zod";

// Zod v4 schema def types that have child schemas.
// Verified against Zod 4.3.6 internals — every compound type's def
// shape and child property name is listed here.
const CHILD_ACCESSORS: Record<string, (def: any) => z.ZodType[]> = {
  object: (def) => Object.values(def.shape),
  array: (def) => [def.element],
  tuple: (def) => [...def.items, ...(def.rest ? [def.rest] : [])],
  union: (def) => def.options,
  intersection: (def) => [def.left, def.right],
  record: (def) => [def.keyType, def.valueType],
  // Wrappers with a single inner type
  nullable: (def) => [def.innerType],
  optional: (def) => [def.innerType],
  nonoptional: (def) => [def.innerType],
  default: (def) => [def.innerType],
  catch: (def) => [def.innerType],
  readonly: (def) => [def.innerType],
  promise: (def) => [def.innerType],
  // Pipe has two children
  pipe: (def) => [def.in, def.out],
  // Lazy resolves to inner schema
  lazy: (def) => [def.getter()],
};

/**
 * Walk the Zod schema tree and reject patterns that `toJSONSchema()`
 * handles incorrectly or silently drops:
 *
 * - `z.intersection()` — produces `allOf` with `additionalProperties: false`
 *   on both sides (from `io: "output"`), making the intersection unmatchable
 *   on Draft 7.
 *
 * - `.refine()` / `.superRefine()` — silently stripped from JSON Schema
 *   output, so the Rust side would accept values that fail the refinement.
 *   Detected by checking for custom checks (`check._zod.def.check === "custom"`)
 *   in the schema's checks array.
 */
function assertNoUnsupportedPatterns(
  schema: z.ZodType,
  label: string,
  visited = new WeakSet<z.ZodType>(),
): void {
  if (visited.has(schema)) {
    return;
  }
  visited.add(schema);

  const def = (schema as any)._zod.def;

  // Reject intersections
  if (def.type === "intersection") {
    throw new Error(
      `Handler "${label}": z.intersection() is not supported. ` +
        `It produces broken JSON Schema on Draft 7 because both sides ` +
        `get additionalProperties: false. Use z.object().extend() or ` +
        `z.object().merge() instead.`,
    );
  }

  // Reject custom checks (from .refine() and .superRefine())
  const checks: any[] | undefined = def.checks;
  if (checks) {
    for (const check of checks) {
      if (check._zod.def.check === "custom") {
        throw new Error(
          `Handler "${label}": .refine() and .superRefine() are not ` +
            `supported. Custom validations cannot be expressed in JSON ` +
            `Schema and would be silently dropped.`,
        );
      }
    }
  }

  // Recurse into children
  const getChildren = CHILD_ACCESSORS[def.type];
  if (getChildren) {
    for (const child of getChildren(def)) {
      assertNoUnsupportedPatterns(child, label, visited);
    }
  }
}

/**
 * Convert a Zod schema to a JSON Schema document suitable for embedding
 * in the serialized AST. Throws if the schema contains types that can't
 * survive the TS → JSON → Rust boundary.
 */
export function zodToCheckedJsonSchema(
  schema: z.ZodType,
  label: string,
): JSONSchema7 {
  // Pre-validate: catch patterns that toJSONSchema() handles incorrectly
  assertNoUnsupportedPatterns(schema, label);

  let raw: Record<string, unknown>;
  try {
    raw = toJSONSchema(schema, {
      target: "draft-07",
      unrepresentable: "throw",
      io: "output",
      cycles: "throw",
      reused: "inline",
    }) as Record<string, unknown>;
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    throw new Error(
      `Handler "${label}": Zod schema cannot be converted to JSON Schema: ${message}`,
      { cause: error },
    );
  }

  // Strip $schema — embedded schemas don't need the draft URI.
  const { $schema: _, ...rest } = raw;
  return rest as JSONSchema7;
}
