import type { JSONSchema7 } from "json-schema";
import { chain } from "./chain.js";
import {
  constant,
  drop,
  extractPrefix,
  flatten as flattenBuiltin,
  getField,
  getIndex,
  identity,
  panic,
  pick,
  splitFirst,
  splitLast,
  tag,
  wrapInField,
  asOption as asOptionStandalone,
} from "./builtins/index.js";
import { Option } from "./option.js";
import { Result } from "./result.js";
// Lazy import — iterator.ts imports from ast.ts, but these are only called inside
// methods (after all modules load), so the circular reference is safe at runtime.
import { Iterator as IteratorNs } from "./iterator.js";
// Lazy import — bind.ts imports from ast.ts, but these are only called inside
// methods (after all modules load), so the circular reference is safe at runtime.
import {
  bind as bindStandalone,
  bindInput as bindInputStandalone,
  type VarRef,
  type InferVarRefs,
} from "./bind.js";

// ---------------------------------------------------------------------------
// Serializable Types — mirror the Rust AST in barnum_ast
// ---------------------------------------------------------------------------

export type Action =
  | InvokeAction
  | ChainAction
  | ForEachAction
  | AllAction
  | BranchAction
  | ResumeHandleAction
  | ResumePerformAction
  | RestartHandleAction
  | RestartPerformAction;

export interface InvokeAction {
  kind: "Invoke";
  handler: HandlerKind;
}

export interface ChainAction {
  kind: "Chain";
  first: Action;
  rest: Action;
}

export interface ForEachAction {
  kind: "ForEach";
  action: Action;
}

export interface AllAction {
  kind: "All";
  actions: Action[];
}

export interface BranchAction {
  kind: "Branch";
  cases: Record<string, Action>;
}

export interface ResumeHandleAction {
  kind: "ResumeHandle";
  resume_handler_id: ResumeHandlerId;
  body: Action;
  handler: Action;
}

export interface ResumePerformAction {
  kind: "ResumePerform";
  resume_handler_id: ResumeHandlerId;
}

export interface RestartHandleAction {
  kind: "RestartHandle";
  restart_handler_id: RestartHandlerId;
  body: Action;
  handler: Action;
}

export interface RestartPerformAction {
  kind: "RestartPerform";
  restart_handler_id: RestartHandlerId;
}

// ---------------------------------------------------------------------------
// HandlerKind
// ---------------------------------------------------------------------------

export type HandlerKind = TypeScriptHandler | BuiltinHandler;

export interface TypeScriptHandler {
  kind: "TypeScript";
  module: string;
  func: string;
  input_schema?: JSONSchema7;
  output_schema?: JSONSchema7;
}

export interface BuiltinHandler {
  kind: "Builtin";
  builtin: BuiltinKind;
}

export type BuiltinKind =
  | { kind: "Constant"; value: unknown }
  | { kind: "Identity" }
  | { kind: "Drop" }
  | { kind: "Merge" }
  | { kind: "Flatten" }
  | { kind: "GetField"; field: string }
  | { kind: "GetIndex"; index: number }
  | { kind: "CollectSome" }
  // TODO: Add WrapInArray builtin (T → [T]). Currently done via all(identity()) which
  // works but routes through the All executor for a trivial operation.
  | { kind: "AsOption" }
  | { kind: "SplitFirst" }
  | { kind: "SplitLast" }
  | { kind: "WrapInField"; field: string }
  | { kind: "Sleep"; ms: number }
  | { kind: "Panic"; message: string }
  | { kind: "ExtractPrefix" };

/**
 * When T is `never` or `void` (handler ignores input / recur doesn't
 * thread state), produce `any` so the combinator can sit in any
 * pipeline position.
 */
export type PipeIn<T> = [T] extends [never] ? any : [T] extends [void] ? any : T;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

export interface Config {
  workflow: Action;
}

// ---------------------------------------------------------------------------
// Type utilities
// ---------------------------------------------------------------------------

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type UnionToIntersection<TUnion> = (
  TUnion extends any ? (x: TUnion) => void : never
) extends (x: infer TIntersection) => void
  ? TIntersection
  : never;

/** Merge a tuple of objects into a single intersection type. */
export type MergeTuple<TTuple> = TTuple extends unknown[]
  ? UnionToIntersection<TTuple[number]>
  : never;

// ---------------------------------------------------------------------------
// Phantom Types — type-safe input/output tracking
// ---------------------------------------------------------------------------

/**
 * An action with tracked input/output types. Phantom fields enforce variance
 * and are never set at runtime — they exist only for the TypeScript compiler.
 *
 *   In:  __in (contravariant) + __in_co (covariant) → invariant
 *   Out: __out (covariant only)
 *
 * Input invariance ensures exact type matching at pipeline connection points.
 * Data crosses serialization boundaries to handlers in arbitrary languages
 * (Rust, Python, etc.), so extra/missing fields are runtime errors.
 *
 * Output covariance is safe — a step producing Dog where Animal is expected
 * downstream works. `never` (throwError, recur, done) is assignable to any
 * output slot via standard subtyping.
 */
export type TypedAction<In = unknown, Out = unknown> = Action & {
  __in?: (input: In) => void;
  __in_co?: In;
  __out?: () => Out;
  /** Chain this action with another. `a.then(b)` ≡ `chain(a, b)`. */
  then<TNext>(next: Pipeable<Out, TNext>): TypedAction<In, TNext>;
  /** Apply an action to each element of an array output. `a.forEach(b)` ≡ `a.then(forEach(b))`. */
  forEach<TIn, TElement, TNext>(
    this: TypedAction<TIn, TElement[]>,
    action: Pipeable<TElement, TNext>,
  ): TypedAction<TIn, TNext[]>;
  /** Dispatch on a tagged union output. Auto-unwraps `value` before each case handler. */
  branch<
    TCases extends {
      [K in BranchKeys<Out>]: CaseHandler<BranchPayload<Out, K>, unknown>;
    },
  >(
    cases: [BranchKeys<Out>] extends [never] ? never : TCases,
  ): TypedAction<In, ExtractOutput<TCases[keyof TCases & string]>>;
  /** Flatten one level of array nesting. `TElement[][] → TElement[]` */
  flatten<TIn, TElement>(
    this: TypedAction<TIn, TElement[][]>,
  ): TypedAction<TIn, TElement[]>;
  /** Discard output. `a.drop()` ≡ `pipe(a, drop)`. */
  drop(): TypedAction<In, void>;
  /** Wrap output as a tagged union member. Requires full variant map TDef so __def is carried. */
  tag<TEnumName extends string, TDef extends Record<string, unknown>, TKind extends keyof TDef & string>(
    kind: TKind,
    enumName: TEnumName,
  ): TypedAction<In, TaggedUnion<TEnumName, TDef>>;
  /** Wrap output as `Option.Some`. `T → Option<T>` */
  some(): TypedAction<In, Option<Out>>;
  /** Wrap output as `Result.Ok`. `T → Result<T, never>` */
  ok(): TypedAction<In, Result<Out, never>>;
  /** Wrap output as `Result.Err`. `T → Result<never, T>` */
  err(): TypedAction<In, Result<never, Out>>;
  /** Extract a field from the output object. `a.getField("name")` ≡ `pipe(a, getField("name"))`. */
  getField<TField extends keyof Out & string>(
    field: TField,
  ): TypedAction<In, Out[TField]>;
  /** Extract an element from the output array by index. Returns Option. */
  getIndex<TIn, TTuple extends unknown[], TIndex extends number>(
    this: TypedAction<TIn, TTuple>,
    index: TIndex,
  ): TypedAction<TIn, Option<TTuple[TIndex]>>;
  /** Wrap output in an object under a field name. `a.wrapInField("foo")` ≡ `pipe(a, wrapInField("foo"))`. */
  wrapInField<TField extends string>(
    field: TField,
  ): TypedAction<In, Record<TField, Out>>;
/** Select fields from the output. `a.pick("x", "y")` ≡ `pipe(a, pick("x", "y"))`. */
  pick<TKeys extends (keyof Out & string)[]>(
    ...keys: TKeys
  ): TypedAction<In, Pick<Out, TKeys[number]>>;
  /** Head/tail decomposition. Only callable when Out is TElement[]. */
  splitFirst<TIn, TElement>(
    this: TypedAction<TIn, TElement[]>,
  ): TypedAction<TIn, Option<[TElement, TElement[]]>>;
  /** Init/last decomposition. Only callable when Out is TElement[]. */
  splitLast<TIn, TElement>(
    this: TypedAction<TIn, TElement[]>,
  ): TypedAction<TIn, Option<[TElement[], TElement]>>;
  /**
   * Transform the inner value. Dispatches: Option.map, Result.map.
   */
  map<TIn, T, U>(
    this: TypedAction<TIn, Option<T>>,
    action: Pipeable<T, U>,
  ): TypedAction<TIn, Option<U>>;
  map<TIn, TValue, TOut, TError>(
    this: TypedAction<TIn, Result<TValue, TError>>,
    action: Pipeable<TValue, TOut>,
  ): TypedAction<TIn, Result<TOut, TError>>;
  /**
   * Transform the Err value of a Result output.
   * `Result<TValue, TError> → Result<TValue, TErrorOut>`
   */
  mapErr<TIn, TValue, TError, TErrorOut>(
    this: TypedAction<TIn, Result<TValue, TError>>,
    action: Pipeable<TError, TErrorOut>,
  ): TypedAction<TIn, Result<TValue, TErrorOut>>;
  /**
   * Unwrap or panic. Dispatches: Option.unwrap, Result.unwrap.
   *
   * Option: If Some, pass through value. If None, panic.
   * Result: If Ok, pass through value. If Err, panic.
   */
  unwrap<TIn, TValue>(
    this: TypedAction<TIn, Option<TValue>>,
  ): TypedAction<TIn, TValue>;
  unwrap<TIn, TValue, TError>(
    this: TypedAction<TIn, Result<TValue, TError>>,
  ): TypedAction<TIn, TValue>;

  /**
   * Unwrap a union output. Dispatches: Option.unwrapOr, Result.unwrapOr.
   *
   * Option: If Some, pass through value. If None, apply default.
   * Result: If Ok, pass through value. If Err, apply default.
   *
   * Covariant output makes throw tokens (Out=never) work:
   *   `handler.unwrapOr(throwError)`
   */
  unwrapOr<TIn, TValue>(
    this: TypedAction<TIn, Option<TValue>>,
    defaultAction: Pipeable<void, TValue>,
  ): TypedAction<TIn, TValue>;
  unwrapOr<TIn, TValue, TError>(
    this: TypedAction<TIn, Result<TValue, TError>>,
    defaultAction: Pipeable<TError, TValue>,
  ): TypedAction<TIn, TValue>;

  /** Monadic bind. Option: `Option<T> → Option<U>`. Result: `Result<T,E> → Result<U,E>`. */
  andThen<TIn, TValue, TOut>(
    this: TypedAction<TIn, Option<TValue>>,
    action: Pipeable<TValue, Option<TOut>>,
  ): TypedAction<TIn, Option<TOut>>;
  andThen<TIn, TValue, TOut, TError>(
    this: TypedAction<TIn, Result<TValue, TError>>,
    action: Pipeable<TValue, Result<TOut, TError>>,
  ): TypedAction<TIn, Result<TOut, TError>>;

  /** Conditional keep. If Some, apply predicate. If None, stay None. */
  filter<TIn, TValue>(
    this: TypedAction<TIn, Option<TValue>>,
    predicate: Pipeable<TValue, Option<TValue>>,
  ): TypedAction<TIn, Option<TValue>>;

  /** Test if the value is Some. `Option<T> → boolean` */
  isSome<TIn, TValue>(
    this: TypedAction<TIn, Option<TValue>>,
  ): TypedAction<TIn, boolean>;

  /** Test if the value is None. `Option<T> → boolean` */
  isNone<TIn, TValue>(
    this: TypedAction<TIn, Option<TValue>>,
  ): TypedAction<TIn, boolean>;

  /** Collect Some values from an array, discarding Nones. `Option<T>[] → T[]` */
  collect<TIn, TValue>(
    this: TypedAction<TIn, Option<TValue>[]>,
  ): TypedAction<TIn, TValue[]>;

  /** Fallback on Err. `Result<T,E> → Result<T,F>` */
  or<TIn, TValue, TError, TErrorOut>(
    this: TypedAction<TIn, Result<TValue, TError>>,
    fallback: Pipeable<TError, Result<TValue, TErrorOut>>,
  ): TypedAction<TIn, Result<TValue, TErrorOut>>;


  /** Convert Ok to Some, Err to None. `Result<T,E> → Option<T>` */
  asOkOption<TIn, TValue, TError>(
    this: TypedAction<TIn, Result<TValue, TError>>,
  ): TypedAction<TIn, Option<TValue>>;

  /** Convert Err to Some, Ok to None. `Result<T,E> → Option<E>` */
  asErrOption<TIn, TValue, TError>(
    this: TypedAction<TIn, Result<TValue, TError>>,
  ): TypedAction<TIn, Option<TError>>;

  /** Convert boolean to Option<void>. `boolean → Option<void>` */
  asOption<TIn>(
    this: TypedAction<TIn, boolean>,
  ): TypedAction<TIn, Option<void>>;

  /** Test if the value is Ok. `Result<T,E> → boolean` */
  isOk<TIn, TValue, TError>(
    this: TypedAction<TIn, Result<TValue, TError>>,
  ): TypedAction<TIn, boolean>;

  /** Test if the value is Err. `Result<T,E> → boolean` */
  isErr<TIn, TValue, TError>(
    this: TypedAction<TIn, Result<TValue, TError>>,
  ): TypedAction<TIn, boolean>;

  /** Swap nesting. `Option<Result<T,E>> → Result<Option<T>,E>` or `Result<Option<T>,E> → Option<Result<T,E>>`. */
  transpose<TIn, TValue, TError>(
    this: TypedAction<TIn, Option<Result<TValue, TError>>>,
  ): TypedAction<TIn, Result<Option<TValue>, TError>>;
  transpose<TIn, TValue, TError>(
    this: TypedAction<TIn, Result<Option<TValue>, TError>>,
  ): TypedAction<TIn, Option<Result<TValue, TError>>>;

  // --- Iterator methods ---

  /** Enter Iterator from Option. `Option<T> → Iterator<T>` */
  iterate<TIn, TElement>(
    this: TypedAction<TIn, Option<TElement>>,
  ): TypedAction<TIn, Iterator<TElement>>;
  /** Enter Iterator from Result. `Result<T,E> → Iterator<T>` */
  iterate<TIn, TElement, TError>(
    this: TypedAction<TIn, Result<TElement, TError>>,
  ): TypedAction<TIn, Iterator<TElement>>;
  /** Enter Iterator from array. `T[] → Iterator<T>` */
  iterate<TIn, TElement>(
    this: TypedAction<TIn, TElement[]>,
  ): TypedAction<TIn, Iterator<TElement>>;

  /** Transform each element in Iterator. `Iterator<T> → Iterator<U>` */
  map<TIn, TElement, TOut>(
    this: TypedAction<TIn, Iterator<TElement>>,
    action: Pipeable<TElement, TOut>,
  ): TypedAction<TIn, Iterator<TOut>>;

  /** Flat-map each element. `f` returns Iterator. `Iterator<T> → Iterator<U>` */
  flatMap<TIn, TElement, TOut>(
    this: TypedAction<TIn, Iterator<TElement>>,
    action: Pipeable<TElement, Iterator<TOut>>,
  ): TypedAction<TIn, Iterator<TOut>>;
  /** Flat-map each element. `f` returns Option. `Iterator<T> → Iterator<U>` */
  flatMap<TIn, TElement, TOut>(
    this: TypedAction<TIn, Iterator<TElement>>,
    action: Pipeable<TElement, Option<TOut>>,
  ): TypedAction<TIn, Iterator<TOut>>;
  /** Flat-map each element. `f` returns Result. `Iterator<T> → Iterator<U>` */
  flatMap<TIn, TElement, TOut, TError>(
    this: TypedAction<TIn, Iterator<TElement>>,
    action: Pipeable<TElement, Result<TOut, TError>>,
  ): TypedAction<TIn, Iterator<TOut>>;
  /** Flat-map each element. `f` returns array. `Iterator<T> → Iterator<U>` */
  flatMap<TIn, TElement, TOut>(
    this: TypedAction<TIn, Iterator<TElement>>,
    action: Pipeable<TElement, TOut[]>,
  ): TypedAction<TIn, Iterator<TOut>>;

  /** Keep elements where predicate returns true. `Iterator<T> → Iterator<T>` */
  filter<TIn, TElement>(
    this: TypedAction<TIn, Iterator<TElement>>,
    predicate: Pipeable<TElement, boolean>,
  ): TypedAction<TIn, Iterator<TElement>>;

  /** Unwrap Iterator to array. `Iterator<T> → T[]` */
  collect<TIn, TElement>(
    this: TypedAction<TIn, Iterator<TElement>>,
  ): TypedAction<TIn, TElement[]>;

  /** Bind concurrent values as VarRefs available throughout the body. */
  bind<TBindings extends Action[], TOut>(
    bindings: [...TBindings],
    body: (vars: InferVarRefs<TBindings>) => Action & { __out?: () => TOut },
  ): TypedAction<In, TOut>;
  /** Capture the pipeline input as a VarRef. */
  bindInput<TOut>(
    body: (input: VarRef<Out>) => Action & { __out?: () => TOut },
  ): TypedAction<In, TOut>;
};

/**
 * Parameter type for pipe and combinators. Same phantom fields as TypedAction
 * but without methods.
 *
 * Why no methods: TypedAction's methods (then, branch, etc.) participate in
 * TS assignability checks in complex, recursive ways that interfere with
 * generic inference in pipe overloads. Pipeable strips methods so that only
 * phantom fields drive inference.
 *
 * TypedAction (with methods) is assignable to Pipeable because Pipeable
 * only requires a subset of properties.
 */
export type Pipeable<In = unknown, Out = unknown> = Action & {
  __in?: (input: In) => void;
  __in_co?: In;
  __out?: () => Out;
};

/**
 * Strip phantom types from a Pipeable, returning a plain Action.
 *
 * Replaces `x as Action` casts throughout the codebase. The constraint
 * ensures the argument is structurally a Pipeable — unlike a bare cast,
 * `toAction(123)` is a type error.
 */
export function toAction<TIn, TOut>(pipeable: Pipeable<TIn, TOut>): Action {
  return pipeable;
}

/**
 * Contravariant input + covariant output for branch case handler positions.
 *
 * Omits __in_co (covariant input) compared to Pipeable. This gives:
 *   In:  contravariant only (via __in)
 *   Out: covariant only (via __out)
 *
 * Why contravariant input: a handler that accepts `unknown` (like drop)
 * can handle any variant. (input: unknown) => void is assignable to
 * (input: HasErrors) => void because HasErrors extends unknown.
 * Pipeable's invariant input (__in_co) would reject this.
 *
 * TypedAction is assignable to CaseHandler because CaseHandler only
 * requires a subset of TypedAction's phantom fields.
 */
type CaseHandler<TIn = unknown, TOut = unknown> = Action & {
  __in?: (input: TIn) => void;
  __out?: () => TOut;
};

// ---------------------------------------------------------------------------
// Tagged Union — standard { kind, value } convention with phantom __def
// ---------------------------------------------------------------------------

/**
 * Standard tagged union type. Each variant is `{ kind: K; value: TDef[K] }`
 * with a phantom `__def` field carrying the full variant map. The __def
 * field enables `.branch()` to decompose the union via simple indexing
 * (`keyof ExtractDef<Out>` and `ExtractDef<Out>[K]`) instead of
 * conditional types (`KindOf<Out>` and `Extract<Out, { kind: K }>`).
 *
 * **Void → null mapping:** Variants with `void` payload (e.g. `{ None: void }`)
 * become `{ kind: "None"; value: null }` at runtime. This is handled by
 * `VoidToNull` below — `void` has no runtime representation in JSON, so it
 * serializes as `null`. Use `z.null()` in Zod schemas for void variants.
 */
// 0 extends 1 & T detects `any` — preserve as-is to avoid collapsing.
type VoidToNull<T> = 0 extends 1 & T
  ? T
  : [T] extends [never]
    ? never
    : [T] extends [void]
      ? null
      : T;

export type TaggedUnion<TEnumName extends string, TDef extends Record<string, unknown>> = {
  [K in keyof TDef & string]: {
    kind: `${TEnumName}.${K}`;
    value: VoidToNull<TDef[K]>;
    __def?: TDef;
  };
}[keyof TDef & string];

/** Extract the variant map definition from a tagged union's phantom __def. */
export type ExtractDef<T> = T extends { __def?: infer D } ? D : never;

// ---------------------------------------------------------------------------
// Option<T> — standard optional value type
// ---------------------------------------------------------------------------

export type OptionDef<T> = { Some: T; None: void };
export type Option<T> = TaggedUnion<"Option", OptionDef<T>>;

// ---------------------------------------------------------------------------
// Result<TValue, TError> — standard success/error type
// ---------------------------------------------------------------------------

export type ResultDef<TValue, TError> = { Ok: TValue; Err: TError };
export type Result<TValue, TError> = TaggedUnion<"Result", ResultDef<TValue, TError>>;

// ---------------------------------------------------------------------------
// Iterator<T> — sequence wrapper (single-variant TaggedUnion)
// ---------------------------------------------------------------------------

export type IteratorDef<TElement> = { Iterator: TElement[] };
export type Iterator<TElement> = TaggedUnion<"Iterator", IteratorDef<TElement>>;

/** Extract all `kind` string literals from a discriminated union. */
type KindOf<T> = T extends { kind: infer K extends string } ? K : never;

/** Strip a `"Prefix."` namespace from a dotted kind string. `"Nat.Zero"` → `"Zero"`. */
type StripKindPrefix<K extends string> = K extends `${string}.${infer Bare}` ? Bare : K;

/** Extract the `value` field from a `{ kind, value }` variant. Falls back to T if no `value` field. */
type UnwrapVariant<T> = T extends { value: infer V } ? V : T;

/**
 * Branch case keys: prefer ExtractDef (simple keyof indexing) when the
 * output carries __def. Falls back to KindOf with prefix stripping for
 * outputs without __def (namespaced kinds like "Nat.Zero" → "Zero").
 */
type BranchKeys<Out> = [ExtractDef<Out>] extends [never]
  ? StripKindPrefix<KindOf<Out>>
  : keyof ExtractDef<Out> & string;

/**
 * Branch case payload: prefer ExtractDef[K] (simple indexing) when available.
 * Falls back to UnwrapVariant<Extract<Out, { kind: ... }>> for outputs without __def.
 * In the fallback, matches namespaced kinds (`"Prefix.K"`) against the bare key K.
 */
type BranchPayload<Out, K extends string> = [ExtractDef<Out>] extends [never]
  ? UnwrapVariant<Extract<Out, { kind: K } | { kind: `${string}.${K}` }>>
  : K extends keyof ExtractDef<Out>
    ? VoidToNull<ExtractDef<Out>[K]>
    : never;

// ---------------------------------------------------------------------------
// typedAction — attach .then() and .forEach() as non-enumerable methods
// ---------------------------------------------------------------------------

// Shared implementations (one closure, not per-instance)
function thenMethod<TIn, TOut, TNext>(
  this: TypedAction<TIn, TOut>,
  next: Pipeable<TOut, TNext>,
): TypedAction<TIn, TNext> {
  return chain(this, next);
}

function forEachMethod(this: TypedAction, action: Action): TypedAction {
  return chain(toAction(this), toAction(forEach(action)));
}

function branchMethod(
  this: TypedAction,
  cases: Record<string, Action>,
): TypedAction {
  return chain(toAction(this), toAction(branch(cases)));
}

function flattenMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(flattenBuiltin()));
}

function dropMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(drop));
}

function tagMethod(this: TypedAction, kind: string, enumName: string): TypedAction {
  return chain(toAction(this), toAction(tag(kind, enumName)));
}

function someMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(Option.some()));
}

function okMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(Result.ok()));
}

function errMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(Result.err()));
}

function getFieldMethod(this: TypedAction, field: string): TypedAction {
  return chain(toAction(this), toAction(getField(field)));
}

function getIndexMethod(this: TypedAction, index: number): TypedAction {
  return chain(toAction(this), toAction(getIndex(index)));
}

function wrapInFieldMethod(this: TypedAction, field: string): TypedAction {
  return chain(toAction(this), toAction(wrapInField(field)));
}

function pickMethod(this: TypedAction, ...keys: string[]): TypedAction {
  return chain(toAction(this), toAction(pick(...keys)));
}

function splitFirstMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(splitFirst()));
}

function splitLastMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(splitLast()));
}

// --- Shared postfix methods (Option + Result) — dispatch via branchFamily ---

function mapMethod(this: TypedAction, action: Action): TypedAction {
  return chain(toAction(this), toAction(branchFamily({
    Result: branch({
      Ok: chain(toAction(action), toAction(Result.ok())),
      Err: Result.err(),
    }),
    Option: branch({
      Some: chain(toAction(action), toAction(Option.some())),
      None: Option.none(),
    }),
    Iterator: IteratorNs.map(action),
  })));
}

function unwrapMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(branchFamily({
    Result: branch({ Ok: identity(), Err: panic("called unwrap on Err") }),
    Option: branch({ Some: identity(), None: panic("called unwrap on None") }),
  })));
}

function unwrapOrMethod(this: TypedAction, defaultAction: Action): TypedAction {
  return chain(toAction(this), toAction(branchFamily({
    Result: branch({ Ok: identity(), Err: defaultAction }),
    Option: branch({ Some: identity(), None: defaultAction }),
  })));
}

function andThenMethod(this: TypedAction, action: Action): TypedAction {
  return chain(toAction(this), toAction(branchFamily({
    Result: branch({ Ok: action, Err: Result.err() }),
    Option: branch({ Some: action, None: Option.none() }),
  })));
}

function transposeMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(branchFamily({
    Option: branch({
      Some: branch({
        Ok: chain(toAction(Option.some()), toAction(Result.ok())),
        Err: Result.err(),
      }),
      None: chain(toAction(chain(toAction(drop), toAction(Option.none()))), toAction(Result.ok())),
    }),
    Result: branch({
      Ok: branch({
        Some: chain(toAction(Result.ok()), toAction(Option.some())),
        None: chain(toAction(drop), toAction(Option.none())),
      }),
      Err: chain(toAction(Result.err()), toAction(Option.some())),
    }),
  })));
}

// --- Result-only postfix methods ---

function mapErrMethod(this: TypedAction, action: Action): TypedAction {
  return chain(toAction(this), toAction(branch({
    Ok: Result.ok(),
    Err: chain(toAction(action), toAction(Result.err())),
  })));
}

function orMethod(this: TypedAction, fallback: Action): TypedAction {
  return chain(toAction(this), toAction(branch({
    Ok: Result.ok(),
    Err: fallback,
  })));
}


function asOkOptionMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(branch({
    Ok: Option.some(),
    Err: chain(toAction(drop), toAction(Option.none())),
  })));
}

function asErrOptionMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(branch({
    Ok: chain(toAction(drop), toAction(Option.none())),
    Err: Option.some(),
  })));
}

function isOkMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(branch({
    Ok: constant(true), Err: constant(false),
  })));
}

function isErrMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(branch({
    Ok: constant(false), Err: constant(true),
  })));
}

// --- Option-only postfix methods ---

function filterMethod(this: TypedAction, predicate: Action): TypedAction {
  return chain(toAction(this), toAction(branchFamily({
    Option: branch({
      Some: predicate,
      None: Option.none(),
    }),
    Iterator: IteratorNs.filter(predicate),
  })));
}

function isSomeMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(branch({
    Some: constant(true), None: constant(false),
  })));
}

function isNoneMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(branch({
    Some: constant(false), None: constant(true),
  })));
}

function asOptionMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(asOptionStandalone()));
}

// --- Iterator postfix methods ---

function iterateMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(branchFamily({
    Option: IteratorNs.fromOption(),
    Result: IteratorNs.fromResult(),
    Array: IteratorNs.fromArray(),
  })));
}

function flatMapMethod(this: TypedAction, action: Action): TypedAction {
  return chain(toAction(this), toAction(IteratorNs.flatMap(action)));
}

function collectMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(branchFamily({
    Array: Option.collect(),
    Iterator: IteratorNs.collect(),
  })));
}

function bindMethod(
  this: TypedAction,
  bindings: Action[],
  body: (vars: any) => Action,
): TypedAction {
  return chain(toAction(this), toAction(bindStandalone(bindings, body)));
}

function bindInputMethod(
  this: TypedAction,
  body: (input: any) => Action,
): TypedAction {
  return chain(toAction(this), toAction(bindInputStandalone(body)));
}

/**
 * Attach `.then()` and `.forEach()` methods to a plain Action object.
 * Methods are non-enumerable: invisible to JSON.stringify and toEqual.
 */
export function typedAction<In = unknown, Out = unknown>(
  action: Action,
): TypedAction<In, Out> {
  if (!("then" in action)) {
    Object.defineProperties(action, {
      then: { value: thenMethod, configurable: true },
      forEach: { value: forEachMethod, configurable: true },
      branch: { value: branchMethod, configurable: true },
      flatten: { value: flattenMethod, configurable: true },
      drop: { value: dropMethod, configurable: true },
      tag: { value: tagMethod, configurable: true },
      some: { value: someMethod, configurable: true },
      ok: { value: okMethod, configurable: true },
      err: { value: errMethod, configurable: true },
      getField: { value: getFieldMethod, configurable: true },
      getIndex: { value: getIndexMethod, configurable: true },
      wrapInField: { value: wrapInFieldMethod, configurable: true },

      pick: { value: pickMethod, configurable: true },
      splitFirst: { value: splitFirstMethod, configurable: true },
      splitLast: { value: splitLastMethod, configurable: true },
      map: { value: mapMethod, configurable: true },
      mapErr: { value: mapErrMethod, configurable: true },
      unwrap: { value: unwrapMethod, configurable: true },
      unwrapOr: { value: unwrapOrMethod, configurable: true },
      andThen: { value: andThenMethod, configurable: true },
      filter: { value: filterMethod, configurable: true },
      isSome: { value: isSomeMethod, configurable: true },
      isNone: { value: isNoneMethod, configurable: true },
      asOption: { value: asOptionMethod, configurable: true },
      collect: { value: collectMethod, configurable: true },
      or: { value: orMethod, configurable: true },
      iterate: { value: iterateMethod, configurable: true },
      flatMap: { value: flatMapMethod, configurable: true },

      asOkOption: { value: asOkOptionMethod, configurable: true },
      asErrOption: { value: asErrOptionMethod, configurable: true },
      isOk: { value: isOkMethod, configurable: true },
      isErr: { value: isErrMethod, configurable: true },
      transpose: { value: transposeMethod, configurable: true },
      bind: { value: bindMethod, configurable: true },
      bindInput: { value: bindInputMethod, configurable: true },
    });
  }
  return action as TypedAction<In, Out>;
}

// ---------------------------------------------------------------------------
// Type extraction utilities
// ---------------------------------------------------------------------------

/**
 * Extract the input type from a TypedAction.
 *
 * Uses direct phantom field extraction (not full TypedAction matching) to
 * avoid a full `TypedAction<any, any>` constraint which fails for In=never
 * due to __in contravariance.
 */
export type ExtractInput<T> = T extends {
  __in?: (input: infer In) => void;
}
  ? In
  : never;

/**
 * Extract the output type from a TypedAction.
 *
 * Uses direct phantom field extraction to avoid constraint issues.
 */
export type ExtractOutput<T> = T extends { __out?: () => infer Out }
  ? Out
  : never;

// ---------------------------------------------------------------------------
// Combinators
// ---------------------------------------------------------------------------

export { pipe } from "./pipe.js";
export { chain } from "./chain.js";
export { all } from "./all.js";
export { bind, bindInput, type VarRef, type InferVarRefs } from "./bind.js";
export { defineRecursiveFunctions } from "./recursive.js";
export { resetEffectIdCounter } from "./effect-id.js";
import {
  allocateRestartHandlerId,
  type RestartHandlerId,
  type ResumeHandlerId,
} from "./effect-id.js";
export { tryCatch } from "./try-catch.js";
export { race, sleep, withTimeout } from "./race.js";

export function forEach<In, Out>(
  action: Pipeable<In, Out>,
): TypedAction<In[], Out[]> {
  return typedAction({ kind: "ForEach", action: toAction(action) });
}

/**
 * Insert GetField("value") before each case handler in a branch.
 * This implements auto-unwrapping: the engine dispatches on `kind`, then
 * extracts `value` before passing to the handler. Case handlers receive
 * the payload directly, not the full `{ kind, value }` variant.
 */
function unwrapBranchCases(
  cases: Record<string, Action>,
): Record<string, Action> {
  const unwrapped: Record<string, Action> = {};
  for (const key of Object.keys(cases)) {
    unwrapped[key] = toAction(chain(
      toAction(getField("value")),
      toAction(cases[key]),
    ));
  }
  return unwrapped;
}

/**
 * Compute the branch input type from its cases. For each case key K,
 * wraps the case handler's input type in `{ kind: K; value: T }`.
 * This ensures the branch input is a proper tagged union matching the
 * `{ kind, value }` convention.
 *
 * Example: `BranchInput<{ Yes: TypedAction<number, ...>, No: TypedAction<string, ...> }>`
 *        = `{ kind: "Yes"; value: number } | { kind: "No"; value: string }`
 *
 * When a case handler uses `any` as input, the wrapping produces
 * `{ kind: K; value: any }`, which is the correct escape hatch.
 */
export type BranchInput<TCases> = {
  [K in keyof TCases & string]: { kind: K; value: ExtractInput<TCases[K]> };
}[keyof TCases & string];

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function branch<TCases extends Record<string, Action>>(
  cases: TCases,
): TypedAction<
  BranchInput<TCases>,
  ExtractOutput<TCases[keyof TCases & string]>
> {
  return typedAction({ kind: "Branch", cases: unwrapBranchCases(cases) });
}

/**
 * Two-level dispatch: extract the enum prefix from a tagged value's `kind`,
 * then branch on that prefix. Used by postfix methods (`.map()`, `.unwrapOr()`,
 * etc.) to dispatch across union families (Option, Result) without runtime
 * metadata.
 *
 * `branchFamily({ Result: ..., Option: ... })` ≡ `chain(extractPrefix(), branch(cases))`
 */
export function branchFamily(cases: Record<string, Action>): TypedAction {
  return typedAction({
    kind: "Chain",
    first: toAction(extractPrefix()),
    rest: toAction(branch(cases)),
  });
}

type LoopResultDef<TContinue, TBreak> = {
  Continue: TContinue;
  Break: TBreak;
};

export type LoopResult<TContinue, TBreak> = TaggedUnion<
  "LoopResult", LoopResultDef<TContinue, TBreak>
>;

// ---------------------------------------------------------------------------
// recur — restart body primitive
// ---------------------------------------------------------------------------

/**
 * Restartable scope. The body callback receives `restart`, a TypedAction that
 * re-executes the body from the beginning with a new input value.
 *
 * If the body completes normally → output is TOut.
 * If restart fires → body re-executes with the restarted value.
 *
 * Compiled form: `RestartHandle(id, GetIndex(0), body)`
 */
export function recur<TIn = void, TOut = any>(
  bodyFn: (restart: TypedAction<TIn, never>) => Pipeable<TIn, TOut>,
): TypedAction<PipeIn<TIn>, TOut> {
  const restartHandlerId = allocateRestartHandlerId();

  const restartAction = typedAction<TIn, never>({
    kind: "RestartPerform",
    restart_handler_id: restartHandlerId,
  });

  const body = toAction(bodyFn(restartAction));

  return typedAction({
    kind: "RestartHandle",
    restart_handler_id: restartHandlerId,
    body,
    handler: toAction(getIndex(0).unwrap()),
  });
}

// ---------------------------------------------------------------------------
// earlyReturn — exit scope primitive (built on restart + Branch)
// ---------------------------------------------------------------------------

/**
 * Early return scope. The body callback receives `earlyReturn`, a TypedAction
 * that exits the scope immediately with the returned value.
 *
 * If the body completes normally → output is TOut.
 * If earlyReturn fires → output is TEarlyReturn.
 * Combined output: TEarlyReturn | TOut.
 *
 * Built on the restart mechanism: input is tagged Continue, body runs inside
 * a Branch. earlyReturn tags with Break and performs — the handler restarts
 * the body, Branch takes the Break path, and the value exits.
 */
export function earlyReturn<TEarlyReturn = void, TIn = any, TOut = any>(
  bodyFn: (
    earlyReturn: TypedAction<TEarlyReturn, never>,
  ) => Pipeable<TIn, TOut>,
): TypedAction<TIn, TEarlyReturn | TOut> {
  const restartHandlerId = allocateRestartHandlerId();

  const earlyReturnAction = typedAction<TEarlyReturn, never>(
    toAction(chain(
      toAction(tag("Break", "LoopResult")),
      { kind: "RestartPerform", restart_handler_id: restartHandlerId },
    )),
  );

  const body = toAction(bodyFn(earlyReturnAction));

  return typedAction(
    buildRestartBranchAction(restartHandlerId, body, toAction(identity())),
  );
}

// ---------------------------------------------------------------------------
// loop — iterative restart with break
// ---------------------------------------------------------------------------

/**
 * Build the restart+branch compiled form:
 * `Chain(Tag("Continue"), RestartHandle(id, GetIndex(0), Branch({ Continue: continueArm, Break: breakArm })))`
 *
 * Input is tagged Continue so the Branch enters the continueArm on first execution.
 * Continue tag → restart → re-enters continueArm. Break tag → restart → runs breakArm, exits `RestartHandle`.
 *
 * Used by earlyReturn, loop, tryCatch, and race.
 */
export function buildRestartBranchAction(
  restartHandlerId: RestartHandlerId,
  continueArm: Action,
  breakArm: Action,
): Action {
  return toAction(chain(
    toAction(tag("Continue", "LoopResult")),
    {
      kind: "RestartHandle",
      restart_handler_id: restartHandlerId,
      body: toAction(branch({ Continue: continueArm, Break: breakArm })),
      handler: toAction(getIndex(0).unwrap()),
    },
  ));
}

/**
 * Iterative loop. The body callback receives `recur` and `done`:
 * - `recur`: restart the loop with a new input
 * - `done`: exit the loop with the break value
 *
 * Both are TypedAction values (not functions), consistent with throwError in tryCatch.
 *
 * Compiles to `RestartHandle`/`RestartPerform`/Branch — same effect substrate as tryCatch and earlyReturn.
 */
export function loop<TBreak = void, TRecur = void>(
  bodyFn: (
    recur: TypedAction<TRecur, never>,
    done: TypedAction<VoidToNull<TBreak>, never>,
  ) => Pipeable<TRecur, never>,
): TypedAction<PipeIn<TRecur>, VoidToNull<TBreak>> {
  const restartHandlerId = allocateRestartHandlerId();

  const perform: Action = {
    kind: "RestartPerform",
    restart_handler_id: restartHandlerId,
  };

  const recurAction = typedAction<TRecur, never>(
    toAction(chain(toAction(tag("Continue", "LoopResult")), toAction(perform))),
  );

  const doneAction = typedAction<VoidToNull<TBreak>, never>(
    toAction(chain(toAction(tag("Break", "LoopResult")), toAction(perform))),
  );

  const body = toAction(bodyFn(recurAction, doneAction));

  return typedAction(
    buildRestartBranchAction(restartHandlerId, body, toAction(identity())),
  );
}

// ---------------------------------------------------------------------------
// Config builders
// ---------------------------------------------------------------------------

/** Simple config factory. */
export function config(workflow: Action): Config {
  return { workflow };
}
