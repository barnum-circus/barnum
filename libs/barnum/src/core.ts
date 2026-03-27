// ---------------------------------------------------------------------------
// Serializable Types — mirror the Rust AST in barnum_ast
// ---------------------------------------------------------------------------

export type Action =
  | CallAction
  | SequenceAction
  | TraverseAction
  | AllAction
  | MatchAction
  | LoopAction
  | AttemptAction
  | StepAction;

export type CallAction = {
  kind: "Call";
  handler: HandlerKind;
};

export type SequenceAction = {
  kind: "Sequence";
  actions: Action[];
};

export type TraverseAction = {
  kind: "Traverse";
  action: Action;
};

export type AllAction = {
  kind: "All";
  actions: Action[];
};

export type MatchAction = {
  kind: "Match";
  cases: Record<string, Action>;
};

export type LoopAction = {
  kind: "Loop";
  body: Action;
};

export type AttemptAction = {
  kind: "Attempt";
  action: Action;
};

export type StepAction = {
  kind: "Step";
  step: string;
};

// ---------------------------------------------------------------------------
// HandlerKind
// ---------------------------------------------------------------------------

export type HandlerKind = TypeScriptHandler;

export type TypeScriptHandler = {
  kind: "TypeScript";
  module: string;
  func: string;
  stepConfig?: unknown;
  valueSchema?: unknown;
};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

export type Config = {
  workflow: Action;
  steps?: Record<string, Action>;
};

// ---------------------------------------------------------------------------
// Phantom Types — type-safe input/output tracking
// ---------------------------------------------------------------------------

/**
 * An action with tracked input/output types. The phantom fields use function
 * types to enforce correct variance (contravariant input, covariant output)
 * and are never set at runtime — they exist only for the TypeScript compiler.
 */
export type TypedAction<In = unknown, Out = unknown> = Action & {
  __phantom_in?: (input: In) => void;
  __phantom_out?: () => Out;
};

/** A handler with tracked input/output types. */
export type TypedHandler<In = unknown, Out = unknown> = HandlerKind & {
  __phantom_in?: (input: In) => void;
  __phantom_out?: () => Out;
};

// ---------------------------------------------------------------------------
// Builders
// ---------------------------------------------------------------------------

export function typescript<In = unknown, Out = unknown>(
  module: string,
  func: string,
): TypedHandler<In, Out> {
  return { kind: "TypeScript", module, func };
}

export function call<In, Out>(
  handler: TypedHandler<In, Out>,
): TypedAction<In, Out> {
  return { kind: "Call", handler };
}

// -- Sequence: type-safe chaining via overloads --

export function sequence<T1, T2>(a1: TypedAction<T1, T2>): TypedAction<T1, T2>;
export function sequence<T1, T2, T3>(
  a1: TypedAction<T1, T2>,
  a2: TypedAction<T2, T3>,
): TypedAction<T1, T3>;
export function sequence<T1, T2, T3, T4>(
  a1: TypedAction<T1, T2>,
  a2: TypedAction<T2, T3>,
  a3: TypedAction<T3, T4>,
): TypedAction<T1, T4>;
export function sequence<T1, T2, T3, T4, T5>(
  a1: TypedAction<T1, T2>,
  a2: TypedAction<T2, T3>,
  a3: TypedAction<T3, T4>,
  a4: TypedAction<T4, T5>,
): TypedAction<T1, T5>;
export function sequence<T1, T2, T3, T4, T5, T6>(
  a1: TypedAction<T1, T2>,
  a2: TypedAction<T2, T3>,
  a3: TypedAction<T3, T4>,
  a4: TypedAction<T4, T5>,
  a5: TypedAction<T5, T6>,
): TypedAction<T1, T6>;
export function sequence<T1, T2, T3, T4, T5, T6, T7>(
  a1: TypedAction<T1, T2>,
  a2: TypedAction<T2, T3>,
  a3: TypedAction<T3, T4>,
  a4: TypedAction<T4, T5>,
  a5: TypedAction<T5, T6>,
  a6: TypedAction<T6, T7>,
): TypedAction<T1, T7>;
export function sequence<T1, T2, T3, T4, T5, T6, T7, T8>(
  a1: TypedAction<T1, T2>,
  a2: TypedAction<T2, T3>,
  a3: TypedAction<T3, T4>,
  a4: TypedAction<T4, T5>,
  a5: TypedAction<T5, T6>,
  a6: TypedAction<T6, T7>,
  a7: TypedAction<T7, T8>,
): TypedAction<T1, T8>;
export function sequence<T1, T2, T3, T4, T5, T6, T7, T8, T9>(
  a1: TypedAction<T1, T2>,
  a2: TypedAction<T2, T3>,
  a3: TypedAction<T3, T4>,
  a4: TypedAction<T4, T5>,
  a5: TypedAction<T5, T6>,
  a6: TypedAction<T6, T7>,
  a7: TypedAction<T7, T8>,
  a8: TypedAction<T8, T9>,
): TypedAction<T1, T9>;
export function sequence<T1, T2, T3, T4, T5, T6, T7, T8, T9, T10>(
  a1: TypedAction<T1, T2>,
  a2: TypedAction<T2, T3>,
  a3: TypedAction<T3, T4>,
  a4: TypedAction<T4, T5>,
  a5: TypedAction<T5, T6>,
  a6: TypedAction<T6, T7>,
  a7: TypedAction<T7, T8>,
  a8: TypedAction<T8, T9>,
  a9: TypedAction<T9, T10>,
): TypedAction<T1, T10>;
export function sequence<T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11>(
  a1: TypedAction<T1, T2>,
  a2: TypedAction<T2, T3>,
  a3: TypedAction<T3, T4>,
  a4: TypedAction<T4, T5>,
  a5: TypedAction<T5, T6>,
  a6: TypedAction<T6, T7>,
  a7: TypedAction<T7, T8>,
  a8: TypedAction<T8, T9>,
  a9: TypedAction<T9, T10>,
  a10: TypedAction<T10, T11>,
): TypedAction<T1, T11>;
export function sequence(...actions: TypedAction[]): TypedAction {
  return { kind: "Sequence", actions };
}

// -- Other builders (untyped for now, type safety added incrementally) --

export function traverse(action: TypedAction): TypedAction {
  return { kind: "Traverse", action };
}

export function all(...actions: TypedAction[]): TypedAction {
  return { kind: "All", actions };
}

export function matchCases(cases: Record<string, TypedAction>): TypedAction {
  return { kind: "Match", cases };
}

export function loop(body: TypedAction): TypedAction {
  return { kind: "Loop", body };
}

export function attempt(action: TypedAction): TypedAction {
  return { kind: "Attempt", action };
}

export function step(name: string): TypedAction {
  return { kind: "Step", step: name };
}

export function config(
  workflow: TypedAction,
  steps?: Record<string, TypedAction>,
): Config {
  const result: Config = { workflow };
  if (steps && Object.keys(steps).length > 0) {
    result.steps = steps;
  }
  return result;
}
