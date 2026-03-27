// ---------------------------------------------------------------------------
// Types — mirror the Rust AST in barnum_ast
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
// Builders
// ---------------------------------------------------------------------------

export function call(handler: HandlerKind): CallAction {
  return { kind: "Call", handler };
}

export function typescript(
  module: string,
  func: string,
): TypeScriptHandler {
  return { kind: "TypeScript", module, func };
}

export function sequence(...actions: Action[]): SequenceAction {
  return { kind: "Sequence", actions };
}

export function traverse(action: Action): TraverseAction {
  return { kind: "Traverse", action };
}

export function all(...actions: Action[]): AllAction {
  return { kind: "All", actions };
}

export function matchCases(cases: Record<string, Action>): MatchAction {
  return { kind: "Match", cases };
}

export function loop(body: Action): LoopAction {
  return { kind: "Loop", body };
}

export function attempt(action: Action): AttemptAction {
  return { kind: "Attempt", action };
}

export function step(name: string): StepAction {
  return { kind: "Step", step: name };
}

export function config(workflow: Action, steps?: Record<string, Action>): Config {
  if (steps && Object.keys(steps).length > 0) {
    return { workflow, steps };
  }
  return { workflow };
}
