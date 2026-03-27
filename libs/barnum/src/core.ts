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

export interface CallAction {
  kind: "Call";
  handler: HandlerKind;
}

export interface SequenceAction {
  kind: "Sequence";
  actions: Action[];
}

export interface TraverseAction {
  kind: "Traverse";
  action: Action;
}

export interface AllAction {
  kind: "All";
  actions: Action[];
}

export interface MatchAction {
  kind: "Match";
  cases: Record<string, Action>;
}

export interface LoopAction {
  kind: "Loop";
  body: Action;
}

export interface AttemptAction {
  kind: "Attempt";
  action: Action;
}

export interface StepAction {
  kind: "Step";
  step: string;
}

// ---------------------------------------------------------------------------
// HandlerKind
// ---------------------------------------------------------------------------

export type HandlerKind = TypeScriptHandler;

export interface TypeScriptHandler {
  kind: "TypeScript";
  module: string;
  func: string;
  stepConfig?: unknown;
  valueSchema?: unknown;
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

export interface Config {
  workflow: Action;
  steps?: Record<string, Action>;
}

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
