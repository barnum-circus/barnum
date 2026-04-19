import type { TaggedUnion, OptionDef, ResultDef, IteratorDef } from "./ast.js";

export * from "./ast.js";
export {
  allObject,
  asOption,
  constant,
  drop,
  flatten,
  getField,
  getIndex,
  identity,

  panic,
  pick,
  range,
  splitFirst,
  splitLast,
  tag,
  taggedUnionSchema,
  withResource,
  wrapInField,
} from "./builtins/index.js";
export { Option, first, last } from "./option.js";
export { Result } from "./result.js";
export { Iterator } from "./iterator.js";
export { runPipeline, type RunPipelineOptions, type LogLevel } from "./run.js";
export { zodToCheckedJsonSchema } from "./schema.js";

// Declaration merge: the explicit value exports of Option/Result from builtins
// shadow the type-only exports from ast's `export *`. Re-declare the generic
// type aliases here so consumers get both the type and value under one name.
export type Option<T> = TaggedUnion<"Option", OptionDef<T>>;
export type Result<TValue, TError> = TaggedUnion<"Result", ResultDef<TValue, TError>>;
export type Iterator<TElement> = TaggedUnion<"Iterator", IteratorDef<TElement>>;
