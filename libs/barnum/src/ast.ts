// ---------------------------------------------------------------------------
// Serializable Types — mirror the Rust AST in barnum_ast
// ---------------------------------------------------------------------------

export type Action =
  | InvokeAction
  | ChainAction
  | ForEachAction
  | ParallelAction
  | BranchAction
  | LoopAction
  | StepAction;

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

export interface ParallelAction {
  kind: "Parallel";
  actions: Action[];
}

export interface BranchAction {
  kind: "Branch";
  cases: Record<string, Action>;
}

export interface LoopAction {
  kind: "Loop";
  body: Action;
}

export interface StepAction {
  kind: "Step";
  step: StepRef;
}

export type StepRef = { kind: "Named"; name: string } | { kind: "Root" };

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
  | { kind: "Pick"; value: string[] };

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export interface Config<Out = any> {
  workflow: TypedAction<never, Out>;
  steps?: Record<string, Action>;
}

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
 * Refs: tracks step reference names through combinators for compile-time
 *   validation in registerSteps (see ValidateStepRefs)
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
  /** Lift this action to operate on arrays. `a.forEach()` ≡ `forEach(a)`. */
  forEach(): TypedAction<In[], Out[], Refs>;
  /** Dispatch on a tagged union output. `a.branch(cases)` ≡ `pipe(a, branch(cases))`. */
  branch<TCases extends Record<string, Action>>(
    cases: TCases,
  ): TypedAction<In, ExtractOutput<TCases[keyof TCases & string]>, Refs | ExtractRefs<TCases[keyof TCases & string]>>;
  /** Flatten a nested array output. `a.flatten()` ≡ `pipe(a, flatten())`. */
  flatten(): TypedAction<In, Out extends (infer TElement)[][] ? TElement[] : Out, Refs>;
  /** Discard output. `a.drop()` ≡ `pipe(a, drop())`. */
  drop(): TypedAction<In, never, Refs>;
  /** Wrap output as a tagged union member. `a.tag("Ok")` ≡ `pipe(a, tag("Ok"))`. */
  tag<TKind extends string>(kind: TKind): TypedAction<In, { kind: TKind; value: Out }, Refs>;
  /** Extract a field from the output object. `a.get("name")` ≡ `pipe(a, extractField("name"))`. */
  get<TField extends keyof Out & string>(field: TField): TypedAction<In, Out[TField], Refs>;
  /**
   * Run this sub-pipeline, then merge its output back into the original input.
   * `pipe(extractField("x"), transform).augment()` takes `In`, runs the
   * sub-pipeline to get `Out`, and returns `In & Out`.
   *
   * Unlike the standalone `augment()` function, the postfix form has access
   * to `In` so the intersection types correctly.
   */
  augment(): TypedAction<In, In & Out, Refs>;
  /** Select fields from the output. `a.pick("x", "y")` ≡ `pipe(a, pick("x", "y"))`. */
  pick<TKeys extends (keyof Out & string)[]>(
    ...keys: TKeys
  ): TypedAction<In, Pick<Out, TKeys[number]>, Refs>;
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


// ---------------------------------------------------------------------------
// typedAction — attach .then() and .forEach() as non-enumerable methods
// ---------------------------------------------------------------------------

// Shared implementations (one closure, not per-instance)
function thenMethod<TIn, TOut, TRefs extends string, TNext, TRefs2 extends string>(
  this: TypedAction<TIn, TOut, TRefs>,
  next: Pipeable<TOut, TNext, TRefs2>,
): TypedAction<TIn, TNext, TRefs | TRefs2> {
  // eslint-disable-next-line @typescript-eslint/no-use-before-define
  return typedAction({ kind: "Chain", first: this, rest: next as Action });
}

function forEachMethod<TIn, TOut, TRefs extends string>(
  this: TypedAction<TIn, TOut, TRefs>,
): TypedAction<TIn[], TOut[], TRefs> {
  // eslint-disable-next-line @typescript-eslint/no-use-before-define
  return typedAction({ kind: "ForEach", action: this });
}

function branchMethod(
  this: TypedAction,
  cases: Record<string, Action>,
): TypedAction {
  // eslint-disable-next-line @typescript-eslint/no-use-before-define
  return typedAction({ kind: "Chain", first: this, rest: { kind: "Branch", cases } });
}

function flattenMethod(this: TypedAction): TypedAction {
  // eslint-disable-next-line @typescript-eslint/no-use-before-define
  return typedAction({
    kind: "Chain",
    first: this,
    rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Flatten" } } },
  });
}

function dropMethod(this: TypedAction): TypedAction {
  // eslint-disable-next-line @typescript-eslint/no-use-before-define
  return typedAction({
    kind: "Chain",
    first: this,
    rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Drop" } } },
  });
}

function tagMethod(this: TypedAction, kind: string): TypedAction {
  // eslint-disable-next-line @typescript-eslint/no-use-before-define
  return typedAction({
    kind: "Chain",
    first: this,
    rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Tag", value: kind } } },
  });
}

function getMethod(this: TypedAction, field: string): TypedAction {
  // eslint-disable-next-line @typescript-eslint/no-use-before-define
  return typedAction({
    kind: "Chain",
    first: this,
    rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "ExtractField", value: field } } },
  });
}

function augmentMethod(this: TypedAction): TypedAction {
  // Construct: Parallel(this, identity) → Merge
  // "this" is the sub-pipeline. augment() wraps it so the original input
  // flows through identity alongside the sub-pipeline, then merges the results.
  // eslint-disable-next-line @typescript-eslint/no-use-before-define
  return typedAction({
    kind: "Chain",
    first: {
      kind: "Parallel",
      actions: [
        this as Action,
        { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Identity" } } },
      ],
    },
    rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Merge" } } },
  });
}

function pickMethod(this: TypedAction, ...keys: string[]): TypedAction {
  // eslint-disable-next-line @typescript-eslint/no-use-before-define
  return typedAction({
    kind: "Chain",
    first: this,
    rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Pick", value: keys } } },
  });
}

/**
 * Attach `.then()` and `.forEach()` methods to a plain Action object.
 * Methods are non-enumerable: invisible to JSON.stringify and toEqual.
 */
export function typedAction<In = unknown, Out = unknown, Refs extends string = never>(
  action: Action,
): TypedAction<In, Out, Refs> {
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
      pick: { value: pickMethod, configurable: true },
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
export type ExtractInput<T> =
  T extends { __phantom_in?: (input: infer In) => void } ? In : never;

/**
 * Extract the output type from a TypedAction.
 *
 * Uses direct phantom field extraction to avoid constraint issues.
 */
export type ExtractOutput<T> =
  T extends { __phantom_out?: () => infer Out } ? Out : never;

/**
 * Extract step reference names tracked in a TypedAction's Refs parameter.
 *
 * Uses direct __refs extraction (not full TypedAction matching) to avoid
 * variance issues with contravariant __phantom_in when In = never.
 */
export type ExtractRefs<T> =
  T extends { __refs?: { _brand: infer R } }
    ? R extends string
      ? R
      : never
    : never;

/**
 * Validates that all step references in R resolve to known step names
 * within the current batch only (keyof R). Previously registered steps
 * should be accessed via the callback's `steps` parameter, not stepRef.
 *
 * When valid: resolves to {} (transparent intersection).
 * When invalid: resolves to a type with __error that causes a compile error.
 */
export type ValidateStepRefs<
  R extends Record<string, Action>,
> = [ExtractRefs<R[keyof R]>] extends [keyof R]
  ? // eslint-disable-next-line @typescript-eslint/no-empty-object-type
    {}
  : {
      __error: `Step reference to undefined step: ${Exclude<ExtractRefs<R[keyof R]>, keyof R> & string}`;
    };

// ---------------------------------------------------------------------------
// Step Reference Tracking (Refs type parameter)
// ---------------------------------------------------------------------------
//
// ## Overview
//
// Named steps can reference each other via `stepRef("B")`. These references
// are validated at compile time: if you write `stepRef("Bt")` when only "A"
// and "B" exist, TypeScript rejects it. This works through a third type
// parameter on TypedAction called `Refs`.
//
// ## How it works
//
// 1. `stepRef<N extends string>(name: N)` returns `TypedAction<any, any, N>`.
//    The literal string "B" is captured in the Refs position.
//
// 2. Every combinator propagates Refs via union. For example, pipe's 2-arg
//    overload is:
//
//      pipe<T1, T2, T3, R1 extends string, R2 extends string>(
//        a1: TypedAction<T1, T2, R1>,
//        a2: TypedAction<T2, T3, R2>,
//      ): TypedAction<T1, T3, R1 | R2>
//
//    So `pipe(check(), stepRef("B"))` has Refs = never | "B" = "B".
//    And `pipe(stepRef("A"), stepRef("B"))` has Refs = "A" | "B".
//
// 3. `registerSteps` uses `ValidateStepRefs` to extract all Refs from the
//    registered step values and check they're a subset of the current batch
//    keys only (keyof R). Previously registered steps are accessed via the
//    typed `steps` parameter in the callback form, not via stepRef:
//
//      registerSteps<R extends Record<string, Action>>(
//        stepsOrBuild:
//          | (R & ValidateStepRefs<R>)
//          | ((ctx: { steps: StripRefs<TSteps>; stepRef: ... })
//              => R & ValidateStepRefs<R>),
//      )
//
//    When valid, ValidateStepRefs resolves to {} (transparent intersection).
//    When invalid, it resolves to { __error: "Step reference to undefined
//    step: Bt" }, which makes the argument incompatible and produces a
//    readable compile error.
//
//    stepRef is not exported — it's only available as a parameter in the
//    registerSteps callback. This ensures step references are always
//    validated within a batch context.
//
// 4. `StripRefs` removes Refs from step types before they're passed to the
//    workflow callback, since refs have already been validated by
//    registerSteps and shouldn't propagate into the workflow's return type.
//
// ## Why Refs is boxed: `__refs?: { _brand: Refs }`
//
// The Refs phantom field uses a boxing wrapper `{ _brand: Refs }` instead of
// a bare `__refs?: Refs`. This is necessary because of how TypeScript infers
// generic type parameters from optional properties on discriminated unions.
//
// When Refs = never (the common case — most actions don't use stepRef), the
// field `__refs?: never` collapses to `undefined` at the type level. When
// pipe's overload tries to infer `R1 extends string` from this field, TS
// sees `undefined`, can't find a valid inference for `R1`, and falls back to
// the constraint bound `string`.
//
// This means `pipe(check(), recur())` — two actions with no step refs —
// would infer Refs = string | string = string. Then ValidateStepRefs sees
// Refs = string and rejects it because `string` doesn't extend the step
// name literals.
//
// **Important**: this only happens with the real Action type (a discriminated
// union of 8 variants). With a simple `{ kind: string }` Action type, TS
// infers correctly. The union distribution changes how TS resolves optional
// fields during inference.
//
// The fix: `__refs?: { _brand: Refs }`. When Refs = never, the field is
// `__refs?: { _brand: never }`. The wrapper `{ _brand: never }` is a
// distinct structural type (not just `undefined`), so TS can match
// `{ _brand: R1 }` against `{ _brand: never }` and correctly infer
// R1 = never.
//
// ## Why ExtractInput/ExtractOutput/ExtractRefs use structural extraction
//
// All three Extract* utilities match on individual phantom fields rather than
// on the full TypedAction type. The constraint `TypedAction<any, any, any>`
// fails for actions with In = never because `(input: never) => void` is not
// assignable to `(input: any) => void` (function params are contravariant).
// Structural extraction avoids this entirely.
//
// ExtractRefs uses a two-step conditional (`infer R` then `R extends string`)
// rather than `infer R extends string` because the latter falls back to the
// constraint bound `string` when R = never.
//
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Combinators
// ---------------------------------------------------------------------------

export { pipe } from "./pipe.js";
export { chain } from "./chain.js";
export { parallel } from "./parallel.js";

export function forEach<In, Out, R extends string = never>(
  action: Pipeable<In, Out, R>,
): TypedAction<In[], Out[], R> {
  return typedAction({ kind: "ForEach", action: action as Action });
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function branch<TCases extends Record<string, Action>>(
  cases: TCases,
): TypedAction<
  ExtractInput<TCases[keyof TCases & string]>,
  ExtractOutput<TCases[keyof TCases & string]>,
  ExtractRefs<TCases[keyof TCases & string]>
> {
  return typedAction({ kind: "Branch", cases });
}

export type LoopResult<TContinue, TBreak> =
  | { kind: "Continue"; value: TContinue }
  | { kind: "Break"; value: TBreak };

/**
 * Extract the Break value type from a LoopResult union.
 *
 * Uses distributive conditional types to pick out the Break member(s)
 * and extract their value type. This is necessary because TypeScript
 * cannot decompose a union during generic inference — `loop<In, TContinue, Out>`
 * would infer both TContinue and Out as the same union, losing the
 * discriminant-based separation. By inferring the body's full output type
 * and then extracting the Break value via a conditional, we get correct results.
 */
type ExtractBreakValue<T> = T extends { kind: "Break"; value: infer V } ? V : never;

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function loop<In, TOut extends LoopResult<any, any>, R extends string = never>(
  body: Pipeable<In, TOut, R>,
): TypedAction<In, ExtractBreakValue<TOut>, R> {
  return typedAction({ kind: "Loop", body: body as Action });
}

/**
 * Create a typed step reference. The name is tracked at the type level
 * via the Refs parameter so registerSteps can validate all references resolve.
 *
 * Not exported — only available as a parameter in registerSteps callbacks.
 * This ensures step references are always validated within a batch context.
 *
 * **Warning: no input/output type safety.** stepRef returns
 * `TypedAction<any, any, N>` — it validates the reference *name* at compile
 * time, but provides no type checking on input or output. The referenced
 * step's types are unknown at the call site (mutual recursion means the
 * step may not be fully defined yet). Prefer `steps.X` from the callback
 * parameter when referencing previously registered steps, as that preserves
 * full input/output types.
 */
function stepRef<N extends string>(name: N): TypedAction<any, any, N> {
  return typedAction({
    kind: "Step",
    step: { kind: "Named", name },
  });
}

// ---------------------------------------------------------------------------
// Config builders
// ---------------------------------------------------------------------------

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type AnyAction = TypedAction<any, any, any>;

/**
 * Strip the Refs parameter from registered step types. Refs are a compile-time
 * mechanism for validating step references in registerSteps — once validated,
 * they shouldn't propagate into the workflow callback's return type.
 */
type StripRefs<TSteps> = {
  [K in keyof TSteps]: TypedAction<ExtractInput<TSteps[K]>, ExtractOutput<TSteps[K]>>;
};

/** Simple config with no named steps. */
export function config<Out>(workflow: TypedAction<never, Out>): Config<Out> {
  return { workflow };
}

/** Builder for configs with type-safe named steps. */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export class ConfigBuilder<TSteps extends Record<string, AnyAction> = {}> {
  private readonly _steps: Record<string, Action>;

  constructor(steps: Record<string, Action> = {}) {
    this._steps = steps;
  }

  /**
   * Register named steps. Two forms:
   *
   * **Object form** — for steps with no cross-references:
   *
   * ```ts
   * .registerSteps({
   *   Setup: setup(),
   *   Migrate: pipe(listFiles(), forEach(migrate())),
   * })
   * ```
   *
   * **Callback form** — for mutual recursion (via `stepRef`) and/or
   * referencing previously registered steps (via `steps`):
   *
   * ```ts
   * .registerSteps({ Setup: setup() })
   * .registerSteps(({ steps, stepRef }) => ({
   *   Pipeline: pipe(steps.Setup, process(), stepRef("FixCycle")),
   *   FixCycle: loop(pipe(check(), stepRef("Pipeline"))),
   * }))
   * ```
   *
   * `stepRef` only validates against current-batch keys. Previously
   * registered steps must be accessed via `steps`.
   */
  registerSteps<R extends Record<string, Action>>(
    steps: R & ValidateStepRefs<R>,
  ): ConfigBuilder<TSteps & R>;
  registerSteps<R extends Record<string, Action>>(
    build: (ctx: {
      steps: StripRefs<TSteps>;
      stepRef: <N extends string>(name: N) => TypedAction<any, any, N>;
    }) => R,
  ): [ExtractRefs<R[keyof R]>] extends [keyof R]
    ? ConfigBuilder<TSteps & R>
    : ValidateStepRefs<R>;
  registerSteps<R extends Record<string, Action>>(
    stepsOrBuild:
      | (R & ValidateStepRefs<R>)
      | ((ctx: {
          steps: StripRefs<TSteps>;
          stepRef: <N extends string>(name: N) => TypedAction<any, any, N>;
        }) => R),
  ): ConfigBuilder<TSteps & R> {
    const resolved =
      typeof stepsOrBuild === "function"
        ? stepsOrBuild({ steps: this._buildStepRefs() as StripRefs<TSteps>, stepRef })
        : stepsOrBuild;
    return new ConfigBuilder({
      ...this._steps,
      ...resolved,
    }) as ConfigBuilder<TSteps & R>;
  }

  /** Build typed step reference objects for previously registered steps. */
  private _buildStepRefs(): Record<string, Action> {
    const refs: Record<string, Action> = {};
    for (const name of Object.keys(this._steps)) {
      refs[name] = typedAction({ kind: "Step", step: { kind: "Named", name } });
    }
    return refs;
  }

  /**
   * Define the workflow entry point.
   *
   * @param build - receives `{ steps, self }`.
   *   `self` is `TypedAction<never, never>` — a jump to the workflow
   *   root. Input `never` because it doesn't consume pipeline data.
   *   Output `never` because the execution path restarts (and `never`
   *   is eliminated from unions, so branches with `self` don't pollute
   *   the output type).
   *
   *   Use `pipe(drop(), self)` to place `self` in a branch case.
   *
   *   Note: ideally `self` would be `TypedAction<never, Out>` so it
   *   carries the workflow's output type, but TypeScript can't infer
   *   a generic from a callback's return and use it in the same
   *   callback's parameter — Out falls back to `unknown`.
   */
  workflow<Out>(
    build: (ctx: {
      steps: StripRefs<TSteps>;
      self: TypedAction<never, never>;
    }) => TypedAction<never, Out>,
  ): RunnableConfig<Out> {
    const stepRefs: Record<string, Action> = {};
    for (const name of Object.keys(this._steps)) {
      stepRefs[name] = typedAction({ kind: "Step", step: { kind: "Named", name } });
    }
    const self = typedAction<never, never>({
      kind: "Step",
      step: { kind: "Root" },
    });
    const workflowAction = build({ steps: stepRefs as StripRefs<TSteps>, self });
    const steps = Object.keys(this._steps).length > 0 ? this._steps : undefined;
    return new RunnableConfig(workflowAction, steps);
  }
}

/**
 * A workflow config with a `.run()` method for execution.
 *
 * Serializes to the same JSON shape as `Config` via `toJSON()`, so it
 * works with `JSON.stringify` and round-trip tests.
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export class RunnableConfig<Out = any> {
  readonly workflow: TypedAction<never, Out>;
  readonly steps?: Record<string, Action>;

  constructor(workflow: TypedAction<never, Out>, steps?: Record<string, Action>) {
    this.workflow = workflow;
    this.steps = steps;
  }

  /** Run this workflow to completion. Prints result to stdout. */
  async run(): Promise<void> {
    // Dynamic import to avoid pulling in Node.js APIs at module load time
    // (keeps ast.ts importable in non-Node environments for type checking).
    const { run } = await import("./run.js");
    run(this.toJSON());
  }

  /** Serialize to the same shape as Config. */
  toJSON(): Config<Out> {
    const result: Config<Out> = { workflow: this.workflow };
    if (this.steps) {
      result.steps = this.steps;
    }
    return result;
  }
}

export function workflowBuilder(): ConfigBuilder {
  return new ConfigBuilder();
}
