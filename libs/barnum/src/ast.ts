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
}

export interface BuiltinHandler {
  kind: "Builtin";
  builtin: BuiltinKind;
}

export type BuiltinKind =
  | { kind: "Constant"; value: unknown }
  | { kind: "Identity" }
  | { kind: "Drop" }
  | { kind: "Tag"; value: string }
  | { kind: "Merge" }
  | { kind: "Flatten" }
  | { kind: "ExtractField"; value: string }
  | { kind: "ExtractIndex"; value: number }
  | { kind: "Pick"; value: string[] }
  | { kind: "CollectSome" };

// ---------------------------------------------------------------------------
// WorkflowAction — loosened input constraint for workflow entry points
// ---------------------------------------------------------------------------

/**
 * A TypedAction suitable as a workflow entry point. Workflows start with
 * no input data, so the action must not require specific input.
 *
 * Uses `__in?: void` to accept both:
 *   - `TypedAction<any, Out>` — combinators that ignore input (constant, sleep)
 *   - `TypedAction<never, Out>` — handlers that genuinely take no params
 *
 * Rejects `TypedAction<{ artifact: string }, Out>` etc. because
 * `{ artifact: string }` is not assignable to `void`.
 *
 * Only `__in` is checked (no `__phantom_in`) — the contravariant phantom
 * field would accept anything due to `void`'s permissiveness, so omitting
 * it is harmless and avoids deep method signature comparison.
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type WorkflowAction<Out = any> = Action & {
  __in?: void;
  __phantom_out?: () => Out;
  __phantom_out_check?: (output: Out) => void;
};

/**
 * When TIn is `never` (handler ignores input), produce `any` so the
 * combinator/pipe can sit in any pipeline position.
 */
export type PipeIn<T> = [T] extends [never] ? any : T;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export interface Config<Out = any> {
  workflow: WorkflowAction<Out>;
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
 * An action with tracked input/output types. Phantom fields enforce invariance
 * and are never set at runtime — they exist only for the TypeScript compiler.
 *
 * Invariance is enforced through paired covariant/contravariant phantom fields:
 *
 *   In:  __phantom_in (contravariant) + __in (covariant) → invariant
 *   Out: __phantom_out (covariant) + __phantom_out_check (contravariant) → invariant
 *
 * This ensures exact type matching at every pipeline connection point.
 * Data crosses serialization boundaries to handlers in arbitrary languages
 * (Rust, Python, etc.), so extra/missing fields are runtime errors.
 *
 * __in also enables config() to reject workflows that expect input
 * (the contravariant __phantom_in makes never the most permissive input,
 * so the covariant __in is needed for the entry point check).
 *
 */
export type TypedAction<
  In = unknown,
  Out = unknown,
  Refs extends string = never,
> = Action & {
  __phantom_in?: (input: In) => void;
  __phantom_out?: () => Out;
  __phantom_out_check?: (output: Out) => void;
  __in?: In;
  __refs?: { _brand: Refs };
  /** Chain this action with another. `a.then(b)` ≡ `chain(a, b)`. */
  then<TNext, TRefs2 extends string = never>(
    next: Pipeable<Out, TNext, TRefs2>,
  ): TypedAction<In, TNext, Refs | TRefs2>;
  /** Apply an action to each element of an array output. `a.forEach(b)` ≡ `a.then(forEach(b))`. */
  forEach<
    TIn,
    TElement,
    TNext,
    TRefs extends string,
    TRefs2 extends string = never,
  >(
    this: TypedAction<TIn, TElement[], TRefs>,
    action: Pipeable<TElement, TNext, TRefs2>,
  ): TypedAction<TIn, TNext[], TRefs | TRefs2>;
  /** Dispatch on a tagged union output. Auto-unwraps `value` before each case handler. */
  branch<
    TCases extends {
      [K in BranchKeys<Out>]: CaseHandler<
        BranchPayload<Out, K>,
        unknown,
        string
      >;
    },
  >(
    cases: [BranchKeys<Out>] extends [never] ? never : TCases,
  ): TypedAction<In, ExtractOutput<TCases[keyof TCases & string]>, Refs>;
  /** Flatten a nested array output. `a.flatten()` ≡ `pipe(a, flatten())`. */
  flatten(): TypedAction<
    In,
    Out extends (infer TElement)[][] ? TElement[] : Out,
    Refs
  >;
  /** Discard output. `a.drop()` ≡ `pipe(a, drop)`. */
  drop(): TypedAction<In, never, Refs>;
  /** Wrap output as a tagged union member. Requires full variant map TDef so __def is carried. */
  tag<TDef extends Record<string, unknown>, TKind extends keyof TDef & string>(
    kind: TKind,
  ): TypedAction<In, TaggedUnion<TDef>, Refs>;
  /** Extract a field from the output object. `a.get("name")` ≡ `pipe(a, extractField("name"))`. */
  get<TField extends keyof Out & string>(
    field: TField,
  ): TypedAction<In, Out[TField], Refs>;
  /**
   * Run this sub-pipeline, then merge its output back into the original input.
   * `pipe(extractField("x"), transform).augment()` takes `In`, runs the
   * sub-pipeline to get `Out`, and returns `In & Out`.
   *
   * Unlike the standalone `augment()` function, the postfix form has access
   * to `In` so the intersection types correctly.
   */
  augment(): TypedAction<In, In & Out, Refs>;
  /** Merge a tuple of objects into a single object. `a.merge()` ≡ `pipe(a, merge())`. */
  merge(): TypedAction<In, MergeTuple<Out>, Refs>;
  /** Select fields from the output. `a.pick("x", "y")` ≡ `pipe(a, pick("x", "y"))`. */
  pick<TKeys extends (keyof Out & string)[]>(
    ...keys: TKeys
  ): TypedAction<In, Pick<Out, TKeys[number]>, Refs>;
  /**
   * Transform the Some value inside an Option output. Only callable when
   * Out is Option<T>. Uses `this` parameter constraint to gate availability.
   */
  mapOption<TIn, T, U, TRefs extends string>(
    this: TypedAction<TIn, Option<T>, TRefs>,
    action: Pipeable<T, U>,
  ): TypedAction<TIn, Option<U>, TRefs>;
  /**
   * Transform the Err value of a Result output.
   * `Result<TValue, TError> → Result<TValue, TErrorOut>`
   *
   * Only callable when Out is Result<TValue, TError>.
   */
  mapErr<TIn, TValue, TError, TErrorOut>(
    this: TypedAction<TIn, Result<TValue, TError>, any>,
    action: Pipeable<TError, TErrorOut>,
  ): TypedAction<TIn, Result<TValue, TErrorOut>, Refs>;
  /**
   * Unwrap a Result output. If Ok, pass through the value. If Err, apply
   * the default action. Only callable when Out is Result<TValue, TError>.
   *
   * The `this` constraint provides TValue from context, so throw tokens
   * (Out=never) work without explicit type parameters:
   *   `handler.unwrapOr(throwError)`
   *
   * Uses CaseHandler for defaultAction (covariant output only) so that
   * `TypedAction<TError, never>` is assignable to `CaseHandler<TError, TValue>`.
   *
   * Refs position uses `any` in the `this` constraint to avoid TS
   * falling back to the constraint bound `string` when Refs = never.
   * The return type uses the enclosing TypedAction's `Refs` directly.
   */
  unwrapOr<TIn, TValue, TError>(
    this: TypedAction<TIn, Result<TValue, TError>, any>,
    defaultAction: CaseHandler<TError, TValue>,
  ): TypedAction<TIn, TValue, Refs>;
};

/**
 * Parameter type for pipe and combinators. Contains the same phantom fields
 * as TypedAction but without methods.
 *
 * Invariance: Both In and Out are invariant, matching TypedAction:
 *   In:  __phantom_in (contravariant) + __in (covariant) → invariant
 *   Out: __phantom_out (covariant) + __phantom_out_check (contravariant) → invariant
 *
 * Why no methods: TypedAction's methods (then, branch, etc.) participate in
 * TS assignability checks in complex, recursive ways that interfere with
 * generic inference in pipe overloads. Pipeable strips methods so that only
 * phantom fields drive inference — predictable covariant/contravariant
 * resolution, with invariance enforced when TS checks candidates from
 * both sides of a connection.
 *
 * TypedAction (with methods) is assignable to Pipeable because Pipeable
 * only requires a subset of properties.
 */
export type Pipeable<
  In = unknown,
  Out = unknown,
  Refs extends string = never,
> = Action & {
  __phantom_in?: (input: In) => void;
  __phantom_out?: () => Out;
  __phantom_out_check?: (output: Out) => void;
  __in?: In;
  __refs?: { _brand: Refs };
};

/**
 * Contravariant-only input checking for branch case handler positions.
 *
 * Omits __in (covariant input) and __phantom_out_check (contravariant output)
 * compared to TypedAction/Pipeable. This gives:
 *   In:  contravariant only (via __phantom_in)
 *   Out: covariant only (via __phantom_out)
 *
 * Why contravariant input: a handler that accepts `unknown` (like drop)
 * can handle any variant. (input: unknown) => void is assignable to
 * (input: HasErrors) => void because HasErrors extends unknown.
 *
 * Why covariant output: the constraint doesn't restrict output types —
 * they're inferred from the actual case handlers via ExtractOutput.
 * TypedAction's invariant __phantom_out_check with Out=unknown would
 * reject any handler with a specific output type, so we omit it.
 *
 * TypedAction is assignable to CaseHandler because CaseHandler only
 * requires a subset of TypedAction's phantom fields.
 */
type CaseHandler<
  TIn = unknown,
  TOut = unknown,
  TRefs extends string = never,
> = Action & {
  __phantom_in?: (input: TIn) => void;
  __phantom_out?: () => TOut;
  __refs?: { _brand: TRefs };
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
 */
export type TaggedUnion<TDef extends Record<string, unknown>> = {
  [K in keyof TDef & string]: { kind: K; value: TDef[K]; __def?: TDef };
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
    ? ExtractDef<Out>[K]
    : never;

// ---------------------------------------------------------------------------
// typedAction — attach .then() and .forEach() as non-enumerable methods
// ---------------------------------------------------------------------------

// Shared implementations (one closure, not per-instance)
function thenMethod<
  TIn,
  TOut,
  TRefs extends string,
  TNext,
  TRefs2 extends string,
>(
  this: TypedAction<TIn, TOut, TRefs>,
  next: Pipeable<TOut, TNext, TRefs2>,
): TypedAction<TIn, TNext, TRefs | TRefs2> {
  return typedAction({ kind: "Chain", first: this, rest: next as Action });
}

function forEachMethod(this: TypedAction, action: Action): TypedAction {
  return typedAction({
    kind: "Chain",
    first: this,
    rest: { kind: "ForEach", action },
  });
}

function branchMethod(
  this: TypedAction,
  cases: Record<string, Action>,
): TypedAction {
  return typedAction({
    kind: "Chain",
    first: this,
    rest: { kind: "Branch", cases: unwrapBranchCases(cases) },
  });
}

function flattenMethod(this: TypedAction): TypedAction {
  return typedAction({
    kind: "Chain",
    first: this,
    rest: {
      kind: "Invoke",
      handler: { kind: "Builtin", builtin: { kind: "Flatten" } },
    },
  });
}

function dropMethod(this: TypedAction): TypedAction {
  return typedAction({
    kind: "Chain",
    first: this,
    rest: {
      kind: "Invoke",
      handler: { kind: "Builtin", builtin: { kind: "Drop" } },
    },
  });
}

function tagMethod(this: TypedAction, kind: string): TypedAction {
  return typedAction({
    kind: "Chain",
    first: this,
    rest: {
      kind: "Invoke",
      handler: { kind: "Builtin", builtin: { kind: "Tag", value: kind } },
    },
  });
}

function getMethod(this: TypedAction, field: string): TypedAction {
  return typedAction({
    kind: "Chain",
    first: this,
    rest: {
      kind: "Invoke",
      handler: {
        kind: "Builtin",
        builtin: { kind: "ExtractField", value: field },
      },
    },
  });
}

function augmentMethod(this: TypedAction): TypedAction {
  // Construct: All(this, identity) → Merge
  // "this" is the sub-pipeline. augment() wraps it so the original input
  // flows through identity alongside the sub-pipeline, then merges the results.
  return typedAction({
    kind: "Chain",
    first: {
      kind: "All",
      actions: [
        this as Action,
        {
          kind: "Invoke",
          handler: { kind: "Builtin", builtin: { kind: "Identity" } },
        },
      ],
    },
    rest: {
      kind: "Invoke",
      handler: { kind: "Builtin", builtin: { kind: "Merge" } },
    },
  });
}

function mergeMethod(this: TypedAction): TypedAction {
  return typedAction({
    kind: "Chain",
    first: this,
    rest: {
      kind: "Invoke",
      handler: { kind: "Builtin", builtin: { kind: "Merge" } },
    },
  });
}

function pickMethod(this: TypedAction, ...keys: string[]): TypedAction {
  return typedAction({
    kind: "Chain",
    first: this,
    rest: {
      kind: "Invoke",
      handler: { kind: "Builtin", builtin: { kind: "Pick", value: keys } },
    },
  });
}

function mapOptionMethod(this: TypedAction, action: Action): TypedAction {
  // Desugars to: self.then(branch({ Some: pipe(action, tag("Some")), None: tag("None") }))
  // But branch auto-unwraps value, so:
  //   Some case: receives T, runs action, wraps as Some
  //   None case: receives void, wraps as None
  return typedAction({
    kind: "Chain",
    first: this,
    rest: {
      kind: "Branch",
      cases: unwrapBranchCases({
        Some: {
          kind: "Chain",
          first: action,
          rest: {
            kind: "Invoke",
            handler: {
              kind: "Builtin",
              builtin: { kind: "Tag", value: "Some" },
            },
          },
        },
        None: {
          kind: "Invoke",
          handler: { kind: "Builtin", builtin: { kind: "Tag", value: "None" } },
        },
      }),
    },
  });
}

function mapErrMethod(this: TypedAction, action: Action): TypedAction {
  // Desugars to: self.then(branch({ Ok: tag("Ok"), Err: pipe(action, tag("Err")) }))
  return typedAction({
    kind: "Chain",
    first: this,
    rest: {
      kind: "Branch",
      cases: unwrapBranchCases({
        Ok: {
          kind: "Invoke",
          handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Ok" } },
        },
        Err: {
          kind: "Chain",
          first: action,
          rest: {
            kind: "Invoke",
            handler: {
              kind: "Builtin",
              builtin: { kind: "Tag", value: "Err" },
            },
          },
        },
      }),
    },
  });
}

function unwrapOrMethod(this: TypedAction, defaultAction: Action): TypedAction {
  // Desugars to: self.then(branch({ Ok: identity(), Err: defaultAction }))
  return typedAction({
    kind: "Chain",
    first: this,
    rest: {
      kind: "Branch",
      cases: unwrapBranchCases({
        Ok: {
          kind: "Invoke",
          handler: { kind: "Builtin", builtin: { kind: "Identity" } },
        },
        Err: defaultAction,
      }),
    },
  });
}

/**
 * Attach `.then()` and `.forEach()` methods to a plain Action object.
 * Methods are non-enumerable: invisible to JSON.stringify and toEqual.
 */
export function typedAction<
  In = unknown,
  Out = unknown,
  Refs extends string = never,
>(action: Action): TypedAction<In, Out, Refs> {
  if (!("then" in action)) {
    Object.defineProperties(action, {
      then: { value: thenMethod, configurable: true },
      forEach: { value: forEachMethod, configurable: true },
      branch: { value: branchMethod, configurable: true },
      flatten: { value: flattenMethod, configurable: true },
      drop: { value: dropMethod, configurable: true },
      tag: { value: tagMethod, configurable: true },
      get: { value: getMethod, configurable: true },
      augment: { value: augmentMethod, configurable: true },
      merge: { value: mergeMethod, configurable: true },
      pick: { value: pickMethod, configurable: true },
      mapOption: { value: mapOptionMethod, configurable: true },
      mapErr: { value: mapErrMethod, configurable: true },
      unwrapOr: { value: unwrapOrMethod, configurable: true },
    });
  }
  return action as TypedAction<In, Out, Refs>;
}

// ---------------------------------------------------------------------------
// Type extraction utilities
// ---------------------------------------------------------------------------

/**
 * Extract the input type from a TypedAction.
 *
 * Uses direct phantom field extraction (not full TypedAction matching) to
 * avoid the `TypedAction<any, any, any>` constraint which fails for In=never
 * due to __phantom_in contravariance.
 */
export type ExtractInput<T> = T extends {
  __phantom_in?: (input: infer In) => void;
}
  ? In
  : never;

/**
 * Extract the output type from a TypedAction.
 *
 * Uses direct phantom field extraction to avoid constraint issues.
 */
export type ExtractOutput<T> = T extends { __phantom_out?: () => infer Out }
  ? Out
  : never;

// ---------------------------------------------------------------------------
// Combinators
// ---------------------------------------------------------------------------

export { pipe } from "./pipe.js";
export { chain } from "./chain.js";
export { all } from "./all.js";
export { bind, bindInput, type VarRef, type InferVarRefs } from "./bind.js";
export { resetEffectIdCounter } from "./effect-id.js";
import {
  allocateRestartHandlerId,
  type RestartHandlerId,
  type ResumeHandlerId,
} from "./effect-id.js";
export { tryCatch } from "./try-catch.js";
export { race, sleep, withTimeout } from "./race.js";

export function forEach<In, Out, R extends string = never>(
  action: Pipeable<In, Out, R>,
): TypedAction<In[], Out[], R> {
  return typedAction({ kind: "ForEach", action: action as Action });
}

/**
 * Insert ExtractField("value") before each case handler in a branch.
 * This implements auto-unwrapping: the engine dispatches on `kind`, then
 * extracts `value` before passing to the handler. Case handlers receive
 * the payload directly, not the full `{ kind, value }` variant.
 */
function unwrapBranchCases(
  cases: Record<string, Action>,
): Record<string, Action> {
  const unwrapped: Record<string, Action> = {};
  for (const key of Object.keys(cases)) {
    unwrapped[key] = {
      kind: "Chain",
      first: {
        kind: "Invoke",
        handler: {
          kind: "Builtin",
          builtin: { kind: "ExtractField", value: "value" },
        },
      },
      rest: cases[key],
    };
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
// Shared AST constants for control flow compilation
// ---------------------------------------------------------------------------

const EXTRACT_PAYLOAD: Action = {
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "ExtractIndex", value: 0 } },
};

const TAG_CONTINUE: Action = {
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Continue" } },
};

export const TAG_BREAK: Action = {
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Break" } },
};

export const IDENTITY: Action = {
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "Identity" } },
};

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
 * Compiled form: `RestartHandle(id, ExtractIndex(0), body)`
 */
export function recur<TIn = never, TOut = any, TRefs extends string = never>(
  bodyFn: (restart: TypedAction<TIn, never>) => Pipeable<TIn, TOut, TRefs>,
): TypedAction<PipeIn<TIn>, TOut, TRefs> {
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
    handler: EXTRACT_PAYLOAD,
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
export function earlyReturn<
  TEarlyReturn = never,
  TIn = any,
  TOut = any,
  TRefs extends string = never,
>(
  bodyFn: (
    earlyReturn: TypedAction<TEarlyReturn, never>,
  ) => Pipeable<TIn, TOut, TRefs>,
): TypedAction<TIn, TEarlyReturn | TOut, TRefs> {
  const restartHandlerId = allocateRestartHandlerId();

  const earlyReturnAction = typedAction<TEarlyReturn, never>({
    kind: "Chain",
    first: TAG_BREAK,
    rest: { kind: "RestartPerform", restart_handler_id: restartHandlerId },
  });

  const body = bodyFn(earlyReturnAction) as Action;

  return typedAction(
    buildRestartBranchAction(restartHandlerId, body, IDENTITY),
  );
}

// ---------------------------------------------------------------------------
// loop — iterative restart with break
// ---------------------------------------------------------------------------

/**
 * Build the restart+branch compiled form:
 * `Chain(Tag("Continue"), RestartHandle(id, ExtractIndex(0), Branch({ Continue: continueArm, Break: breakArm })))`
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
  return {
    kind: "Chain",
    first: TAG_CONTINUE,
    rest: {
      kind: "RestartHandle",
      restart_handler_id: restartHandlerId,
      body: {
        kind: "Branch",
        cases: unwrapBranchCases({
          Continue: continueArm,
          Break: breakArm,
        }),
      },
      handler: EXTRACT_PAYLOAD,
    },
  };
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
export function loop<TBreak = never, TIn = never, TRefs extends string = never>(
  bodyFn: (
    recur: TypedAction<TIn, never>,
    done: TypedAction<TBreak, never>,
  ) => Pipeable<TIn, never, TRefs>,
): TypedAction<PipeIn<TIn>, TBreak, TRefs> {
  const restartHandlerId = allocateRestartHandlerId();

  const perform: Action = {
    kind: "RestartPerform",
    restart_handler_id: restartHandlerId,
  };

  const recurAction = typedAction<TIn, never>({
    kind: "Chain",
    first: TAG_CONTINUE,
    rest: perform,
  });

  const doneAction = typedAction<TBreak, never>({
    kind: "Chain",
    first: TAG_BREAK,
    rest: perform,
  });

  const body = bodyFn(recurAction, doneAction) as Action;

  return typedAction(
    buildRestartBranchAction(restartHandlerId, body, IDENTITY),
  );
}

// ---------------------------------------------------------------------------
// Config builders
// ---------------------------------------------------------------------------

/** Simple config with no named steps. */
export function config<Out>(workflow: WorkflowAction<Out>): Config<Out> {
  return { workflow };
}

/**
 * A workflow config with a `.run()` method for execution.
 *
 * Serializes to the same JSON shape as `Config` via `toJSON()`, so it
 * works with `JSON.stringify` and round-trip tests.
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export class RunnableConfig<Out = any> {
  readonly workflow: WorkflowAction<Out>;

  constructor(workflow: WorkflowAction<Out>) {
    this.workflow = workflow;
  }

  /** Run this workflow to completion. Prints result to stdout. */
  async run(): Promise<void> {
    // Dynamic import to avoid pulling in Node.js APIs at module load time
    // (keeps ast.ts importable in non-Node environments for type checking).
    const { run } = await import("./run.js");
    await run(this.toJSON());
  }

  /** Serialize to the same shape as Config. */
  toJSON(): Config<Out> {
    return { workflow: this.workflow };
  }
}

export interface WorkflowBuilder {
  /** Define the workflow entry point. */
  workflow<Out>(build: () => WorkflowAction<Out>): RunnableConfig<Out>;
}

/** Create a workflow builder. */
export function workflowBuilder(): WorkflowBuilder {
  return {
    workflow<Out>(build: () => WorkflowAction<Out>): RunnableConfig<Out> {
      return new RunnableConfig(build());
    },
  };
}
