import type { TaggedUnion, OptionDef, ResultDef } from "./ast.js";

export * from "./ast.js";
export {
  constant,
  identity,
  drop,
  tag,
  merge,
  flatten,
  extractField,
  extractIndex,
  pick,
  dropResult,
  withResource,
  augment,
  tap,
  range,
  Option,
  Result,
} from "./builtins.js";
export * from "./handler.js";
export { run } from "./run.js";

// Declaration merge: the explicit value exports of Option/Result from builtins
// shadow the type-only exports from ast's `export *`. Re-declare the generic
// type aliases here so consumers get both the type and value under one name.
export type Option<T> = TaggedUnion<OptionDef<T>>;
export type Result<TValue, TError> = TaggedUnion<ResultDef<TValue, TError>>;
