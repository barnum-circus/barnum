import { z } from "zod";
import type { Result, Option } from "./ast.js";

export function resultSchema<TValue, TError>(
  okSchema: z.ZodType<TValue>,
  errSchema: z.ZodType<TError>,
): z.ZodType<Result<TValue, TError>> {
  return z.discriminatedUnion("kind", [
    z.object({ kind: z.literal("Result.Ok"), value: okSchema }),
    z.object({ kind: z.literal("Result.Err"), value: errSchema }),
  ]) as z.ZodType<Result<TValue, TError>>;
}

export function optionSchema<TValue>(
  valueSchema: z.ZodType<TValue>,
): z.ZodType<Option<TValue>> {
  return z.discriminatedUnion("kind", [
    z.object({ kind: z.literal("Option.Some"), value: valueSchema }),
    z.object({ kind: z.literal("Option.None"), value: z.null() }),
  ]) as z.ZodType<Option<TValue>>;
}
