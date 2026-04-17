import {
  type Action,
  type Pipeable,
  type TypedAction,
  toAction,
  typedAction,
  branch,
} from "./ast.js";
import { all } from "./all.js";
import { chain } from "./chain.js";
import { constant, identity, getField, getIndex, tag } from "./builtins/index.js";
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
 * output phantom field — omits __in and __in_co so that actions with
 * any input type (e.g. pipelines starting from a call token) are assignable.
 */
type BodyResult<TOut> = Action & {
  __out?: () => TOut;
};

const UNUSED_STATE = null;

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

  const resumePerform: Action = {
    kind: "ResumePerform",
    resume_handler_id: resumeHandlerId,
  };

  // Call tokens: Chain(Tag("CallN"), ResumePerform(resumeHandlerId))
  const fnCount = bodiesFn.length;
  const callTokens = Array.from({ length: fnCount }, (_, i) =>
    typedAction(toAction(chain(toAction(tag(`Call${i}`, "RecursiveDispatch")), toAction(resumePerform)))),
  );

  // Get function body ASTs
  const bodyActions = (bodiesFn(
    ...(callTokens as FunctionRefs<TDefs>),
  ) as Pipeable[]).map(toAction);

  // Branch cases: CallN → GetField("value") → bodyN
  const cases: Record<string, Action> = {};
  for (let i = 0; i < bodyActions.length; i++) {
    cases[`Call${i}`] = toAction(chain(
      toAction(getField("value")),
      toAction(bodyActions[i]),
    ));
  }

  // Return curried entry-point combinator
  return <TOut>(entryFn: (...fns: FunctionRefs<TDefs>) => BodyResult<TOut>) => {
    const userBody = toAction(entryFn(...(callTokens as FunctionRefs<TDefs>)));

    return typedAction<any, TOut>(
      toAction(chain(toAction(all(identity(), constant(UNUSED_STATE))), {
        kind: "ResumeHandle",
        resume_handler_id: resumeHandlerId,
        body: toAction(chain(toAction(getIndex(0).unwrap()), toAction(userBody))),
        handler: toAction(all(
          chain(toAction(getIndex(0).unwrap()), toAction(branch(cases))),
          constant(UNUSED_STATE),
        )),
      })),
    );
  };
}
