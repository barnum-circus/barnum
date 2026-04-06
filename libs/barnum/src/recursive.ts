import {
  type Action,
  type Pipeable,
  type TypedAction,
  typedAction,
} from "./ast.js";
import { allocateResumeHandlerId } from "./effect-id.js";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

type FunctionDef = [input: unknown, output: unknown];

type FunctionRefs<TDefs extends FunctionDef[]> = {
  [K in keyof TDefs]: TypedAction<TDefs[K][0], TDefs[K][1]>;
};

/**
 * Constraint for the entry-point callback return type. Only requires the
 * output phantom fields — omits __in and __in_co so that actions with
 * In = never (e.g. pipelines starting from a call token) are assignable.
 */
type BodyResult<TOut> = Action & {
  __out?: () => TOut;
  __out_contra?: (output: TOut) => void;
};

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const UNUSED_STATE: any = undefined;

// ---------------------------------------------------------------------------
// defineRecursiveFunctions
// ---------------------------------------------------------------------------

/**
 * Define mutually recursive functions that can call each other.
 *
 * The type parameter is an array of [In, Out] tuples — one per function.
 * TypeScript can't infer these from circular definitions, so they must be
 * explicit.
 *
 * Returns a curried combinator: the first callback defines function bodies,
 * the second receives the same call tokens and returns the workflow entry
 * point.
 *
 * Desugars to a ResumeHandle with a Branch-based handler. Each call token
 * is Chain(Tag("CallN"), ResumePerform(id)). The handler dispatches to the
 * correct function body by tag. The caller's pipeline is preserved as a
 * ResumePerformFrame across each call.
 */
export function defineRecursiveFunctions<TDefs extends FunctionDef[]>(
  bodiesFn: (...fns: FunctionRefs<TDefs>) => {
    [K in keyof TDefs]: Pipeable<TDefs[K][0], TDefs[K][1]>;
  },
): <TOut>(
  entryFn: (...fns: FunctionRefs<TDefs>) => BodyResult<TOut>,
) => TypedAction<any, TOut> {
  const resumeHandlerId = allocateResumeHandlerId();

  // Create call tokens: Chain(Tag("CallN"), ResumePerform(resumeHandlerId))
  const fnCount = bodiesFn.length;
  const callTokens = Array.from({ length: fnCount }, (_, i) =>
    typedAction({
      kind: "Chain",
      first: {
        kind: "Invoke",
        handler: {
          kind: "Builtin",
          builtin: { kind: "Tag", value: `Call${i}` },
        },
      },
      rest: { kind: "ResumePerform", resume_handler_id: resumeHandlerId },
    }),
  );

  // Get function body ASTs
  const bodyActions = bodiesFn(
    ...(callTokens as FunctionRefs<TDefs>),
  ) as Action[];

  // Build Branch cases: CallN → ExtractField("value") → bodyN
  const cases: Record<string, Action> = {};
  for (let i = 0; i < bodyActions.length; i++) {
    cases[`Call${i}`] = {
      kind: "Chain",
      first: {
        kind: "Invoke",
        handler: {
          kind: "Builtin",
          builtin: { kind: "ExtractField", value: "value" },
        },
      },
      rest: bodyActions[i],
    };
  }

  // Return curried entry-point combinator
  return <TOut>(
    entryFn: (...fns: FunctionRefs<TDefs>) => BodyResult<TOut>,
  ) => {
    const userBody = entryFn(
      ...(callTokens as FunctionRefs<TDefs>),
    ) as Action;

    return typedAction<any, TOut>({
      kind: "Chain",
      first: {
        kind: "All",
        actions: [
          {
            kind: "Invoke",
            handler: { kind: "Builtin", builtin: { kind: "Identity" } },
          },
          {
            kind: "Invoke",
            handler: {
              kind: "Builtin",
              builtin: { kind: "Constant", value: UNUSED_STATE },
            },
          },
        ],
      },
      rest: {
        kind: "ResumeHandle",
        resume_handler_id: resumeHandlerId,
        body: {
          kind: "Chain",
          first: {
            kind: "Invoke",
            handler: {
              kind: "Builtin",
              builtin: { kind: "ExtractIndex", value: 0 },
            },
          },
          rest: userBody,
        },
        handler: {
          kind: "All",
          actions: [
            {
              kind: "Chain",
              first: {
                kind: "Invoke",
                handler: {
                  kind: "Builtin",
                  builtin: { kind: "ExtractIndex", value: 0 },
                },
              },
              rest: { kind: "Branch", cases },
            },
            {
              kind: "Invoke",
              handler: {
                kind: "Builtin",
                builtin: { kind: "Constant", value: UNUSED_STATE },
              },
            },
          ],
        },
      },
    });
  };
}
