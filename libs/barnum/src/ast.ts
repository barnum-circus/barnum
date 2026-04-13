import type { JSONSchema7 } from "json-schema";
import { chain } from "./chain.js";
import {
  drop,
  flatten as flattenBuiltin,
  getField,
  getIndex,
  identity,
  merge,
  Option,
  pick,
  Result,
  splitFirst,
  splitLast,
  tag,
  wrapInField,
} from "./builtins.js";
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
  | { kind: "SplitFirst" }
  | { kind: "SplitLast" }
  | { kind: "WrapInField"; field: string }
  | { kind: "Sleep"; ms: number };

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
  /** Flatten a nested array output. `a.flatten()` ≡ `pipe(a, flatten())`. */
  flatten(): TypedAction<
    In,
    Out extends (infer TElement)[][] ? TElement[] : Out
  >;
  /** Discard output. `a.drop()` ≡ `pipe(a, drop)`. */
  drop(): TypedAction<In, void>;
  /** Wrap output as a tagged union member. Requires full variant map TDef so __def is carried. */
  tag<TDef extends Record<string, unknown>, TKind extends keyof TDef & string>(
    kind: TKind,
  ): TypedAction<In, TaggedUnion<TDef>>;
  /** Extract a field from the output object. `a.getField("name")` ≡ `pipe(a, getField("name"))`. */
  getField<TField extends keyof Out & string>(
    field: TField,
  ): TypedAction<In, Out[TField]>;
  /** Extract an element from the output tuple by index. `a.getIndex(0)` ≡ `pipe(a, getIndex(0))`. */
  getIndex<TIn, TTuple extends unknown[], TIndex extends number>(
    this: TypedAction<TIn, TTuple>,
    index: TIndex,
  ): TypedAction<TIn, TTuple[TIndex]>;
  /** Wrap output in an object under a field name. `a.wrapInField("foo")` ≡ `pipe(a, wrapInField("foo"))`. */
  wrapInField<TField extends string>(
    field: TField,
  ): TypedAction<In, Record<TField, Out>>;
  /** Merge a tuple of objects into a single object. `a.merge()` ≡ `pipe(a, merge())`. */
  merge(): TypedAction<In, MergeTuple<Out>>;
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
   * Transform the Some value inside an Option output. Only callable when
   * Out is Option<T>. Uses `this` parameter constraint to gate availability.
   */
  mapOption<TIn, T, U>(
    this: TypedAction<TIn, Option<T>>,
    action: Pipeable<T, U>,
  ): TypedAction<TIn, Option<U>>;
  /**
   * Transform the Err value of a Result output.
   * `Result<TValue, TError> → Result<TValue, TErrorOut>`
   *
   * Only callable when Out is Result<TValue, TError>.
   */
  mapErr<TIn, TValue, TError, TErrorOut>(
    this: TypedAction<TIn, Result<TValue, TError>>,
    action: Pipeable<TError, TErrorOut>,
  ): TypedAction<TIn, Result<TValue, TErrorOut>>;
  /**
   * Unwrap a Result output. If Ok, pass through the value. If Err, apply
   * the default action. Only callable when Out is Result<TValue, TError>.
   *
   * The `this` constraint provides TValue from context, so throw tokens
   * (Out=never) work without explicit type parameters:
   *   `handler.unwrapOr(throwError)`
   *
   * With covariant output, `TypedAction<TError, never>` (throwError, done)
   * is assignable to `Pipeable<TError, TValue>` because `never extends TValue`.
   */
  unwrapOr<TIn, TValue, TError>(
    this: TypedAction<TIn, Result<TValue, TError>>,
    defaultAction: Pipeable<TError, TValue>,
  ): TypedAction<TIn, TValue>;
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

export type TaggedUnion<TDef extends Record<string, unknown>> = {
  [K in keyof TDef & string]: {
    kind: K;
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
export type Option<T> = TaggedUnion<OptionDef<T>>;

// ---------------------------------------------------------------------------
// Result<TValue, TError> — standard success/error type
// ---------------------------------------------------------------------------

export type ResultDef<TValue, TError> = { Ok: TValue; Err: TError };
export type Result<TValue, TError> = TaggedUnion<ResultDef<TValue, TError>>;

/** Extract all `kind` string literals from a discriminated union. */
type KindOf<T> = T extends { kind: infer K extends string } ? K : never;

/** Extract the `value` field from a `{ kind, value }` variant. Falls back to T if no `value` field. */
type UnwrapVariant<T> = T extends { value: infer V } ? V : T;

/**
 * Branch case keys: prefer ExtractDef (simple keyof indexing) when the
 * output carries __def. Falls back to KindOf (conditional type) for
 * outputs without __def.
 */
type BranchKeys<Out> = [ExtractDef<Out>] extends [never]
  ? KindOf<Out>
  : keyof ExtractDef<Out> & string;

/**
 * Branch case payload: prefer ExtractDef[K] (simple indexing) when available.
 * Falls back to UnwrapVariant<Extract<Out, { kind: K }>> for outputs without __def.
 */
type BranchPayload<Out, K extends string> = [ExtractDef<Out>] extends [never]
  ? UnwrapVariant<Extract<Out, { kind: K }>>
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
  return chain(this, next) as TypedAction<TIn, TNext>;
}

function forEachMethod(this: TypedAction, action: Action): TypedAction {
  return chain(this as any, forEach(action as any)) as TypedAction;
}

function branchMethod(
  this: TypedAction,
  cases: Record<string, Action>,
): TypedAction {
  return chain(this as any, branch(cases as any)) as TypedAction;
}

function flattenMethod(this: TypedAction): TypedAction {
  return chain(this as any, flattenBuiltin()) as TypedAction;
}

function dropMethod(this: TypedAction): TypedAction {
  return chain(this as any, drop) as TypedAction;
}

function tagMethod(this: TypedAction, kind: string): TypedAction {
  return chain(this as any, tag(kind)) as TypedAction;
}

function getFieldMethod(this: TypedAction, field: string): TypedAction {
  return chain(this as any, getField(field)) as TypedAction;
}

function getIndexMethod(this: TypedAction, index: number): TypedAction {
  return chain(this as any, getIndex(index)) as TypedAction;
}

function wrapInFieldMethod(this: TypedAction, field: string): TypedAction {
  return chain(this as any, wrapInField(field)) as TypedAction;
}

function mergeMethod(this: TypedAction): TypedAction {
  return chain(this as any, merge()) as TypedAction;
}

function pickMethod(this: TypedAction, ...keys: string[]): TypedAction {
  return chain(this as any, pick(...keys)) as TypedAction;
}

function splitFirstMethod(this: TypedAction): TypedAction {
  return chain(this as any, splitFirst()) as TypedAction;
}

function splitLastMethod(this: TypedAction): TypedAction {
  return chain(this as any, splitLast()) as TypedAction;
}

function mapOptionMethod(this: TypedAction, action: Action): TypedAction {
  return chain(this as any, Option.map(action as any)) as TypedAction;
}

function mapErrMethod(this: TypedAction, action: Action): TypedAction {
  return chain(this as any, Result.mapErr(action as any)) as TypedAction;
}

function unwrapOrMethod(this: TypedAction, defaultAction: Action): TypedAction {
  return chain(
    this as any,
    Result.unwrapOr(defaultAction as any),
  ) as TypedAction;
}

function bindMethod(
  this: TypedAction,
  bindings: Action[],
  body: (vars: any) => Action,
): TypedAction {
  return chain(this as any, bindStandalone(bindings, body) as any) as TypedAction;
}

function bindInputMethod(
  this: TypedAction,
  body: (input: any) => Action,
): TypedAction {
  return chain(this as any, bindInputStandalone(body) as any) as TypedAction;
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
      getField: { value: getFieldMethod, configurable: true },
      getIndex: { value: getIndexMethod, configurable: true },
      wrapInField: { value: wrapInFieldMethod, configurable: true },
      merge: { value: mergeMethod, configurable: true },
      pick: { value: pickMethod, configurable: true },
      splitFirst: { value: splitFirstMethod, configurable: true },
      splitLast: { value: splitLastMethod, configurable: true },
      mapOption: { value: mapOptionMethod, configurable: true },
      mapErr: { value: mapErrMethod, configurable: true },
      unwrapOr: { value: unwrapOrMethod, configurable: true },
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
  return typedAction({ kind: "ForEach", action: action as Action });
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
    unwrapped[key] = chain(
      getField("value") as any,
      cases[key] as any,
    ) as Action;
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

type LoopResultDef<TContinue, TBreak> = {
  Continue: TContinue;
  Break: TBreak;
};

export type LoopResult<TContinue, TBreak> = TaggedUnion<
  LoopResultDef<TContinue, TBreak>
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

  const body = bodyFn(restartAction) as Action;

  return typedAction({
    kind: "RestartHandle",
    restart_handler_id: restartHandlerId,
    body,
    handler: getIndex(0) as Action,
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
    chain(
      tag("Break") as any,
      {
        kind: "RestartPerform",
        restart_handler_id: restartHandlerId,
      } as any,
    ) as Action,
  );

  const body = bodyFn(earlyReturnAction) as Action;

  return typedAction(
    buildRestartBranchAction(restartHandlerId, body, identity() as Action),
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
  return chain(
    tag("Continue") as any,
    {
      kind: "RestartHandle",
      restart_handler_id: restartHandlerId,
      body: branch({ Continue: continueArm, Break: breakArm } as any) as Action,
      handler: getIndex(0) as Action,
    } as any,
  ) as Action;
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
    chain(tag("Continue") as any, perform as any) as Action,
  );

  const doneAction = typedAction<VoidToNull<TBreak>, never>(
    chain(tag("Break") as any, perform as any) as Action,
  );

  const body = bodyFn(recurAction, doneAction) as Action;

  return typedAction(
    buildRestartBranchAction(restartHandlerId, body, identity() as Action),
  );
}

// ---------------------------------------------------------------------------
// Config builders
// ---------------------------------------------------------------------------

/** Simple config factory. */
export function config(workflow: Action): Config {
  return { workflow };
}
