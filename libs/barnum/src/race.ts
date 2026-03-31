import { type Action, type Pipeable, type Result, type TypedAction, typedAction } from "./ast.js";
import { allocateEffectId } from "./effect-id.js";
import { identity } from "./builtins.js";
import { invokeWithThrow } from "./try-catch.js";

// ---------------------------------------------------------------------------
// Shared AST fragments
// ---------------------------------------------------------------------------

const EXTRACT_PAYLOAD: Action = {
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "ExtractField", value: "payload" } },
};

const TAG_DISCARD: Action = {
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Discard" } },
};

const TAG_OK: Action = {
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Ok" } },
};

const TAG_ERR: Action = {
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Err" } },
};

const EXTRACT_VALUE: Action = {
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "ExtractField", value: "value" } },
};

/** Handler DAG shared by race and withTimeout: extract payload, tag Discard. */
const RACE_HANDLER: Action = {
  kind: "Chain",
  first: EXTRACT_PAYLOAD,
  rest: TAG_DISCARD,
};

// ---------------------------------------------------------------------------
// race — first branch to complete wins, losers cancelled
// ---------------------------------------------------------------------------

/**
 * Run multiple actions concurrently. The first to complete wins; losers
 * are cancelled during Handle frame teardown.
 *
 * All branches must have the same input and output type (since either
 * could win).
 *
 * Compiles to:
 *   Handle(effectId, Chain(ExtractField("payload"), Tag("Discard")),
 *     All(
 *       Chain(action1, Perform(effectId)),
 *       Chain(action2, Perform(effectId)),
 *       ...
 *     ),
 *   )
 */
export function race<TIn, TOut>(
  ...actions: Pipeable<TIn, TOut>[]
): TypedAction<TIn, TOut> {
  const effectId = allocateEffectId();

  const perform: Action = { kind: "Perform", effect_id: effectId };

  const branches = actions.map((action) => ({
    kind: "Chain" as const,
    first: action as Action,
    rest: perform,
  }));

  return typedAction({
    kind: "Handle",
    effect_id: effectId,
    handler: RACE_HANDLER,
    body: { kind: "All", actions: branches },
  });
}

// ---------------------------------------------------------------------------
// sleep — TypeScript handler that resolves after N milliseconds
// ---------------------------------------------------------------------------

/**
 * Delay for a specified duration. Returns `void` after the timer fires.
 *
 * Input is `any` — the timer ignores pipeline data. The duration
 * is baked into the AST at build time via handler config.
 *
 * When the engine cancels the sleep during race teardown, the worker
 * subprocess is killed. The sleep never resolves. Standard cancellation.
 *
 * This is defined inline rather than via `createHandlerWithConfig` to avoid
 * a circular dependency (handler.ts → ast.ts → builtins.ts → handler.ts).
 * The handler definition is attached for the worker to find at runtime.
 */
export function sleep(config: { ms: number }): TypedAction<any, void> {
  // We can't use createHandlerWithConfig due to circular deps and because
  // it uses getCallerFilePath (stack trace) which would point to this file.
  // Instead, build the AST directly and attach __definition manually.

  // The Invoke node points to this module's "sleep" export.
  // The worker will import this file and call sleep.__definition.handle().
  const invokeAction: Action = {
    kind: "Invoke",
    handler: {
      kind: "TypeScript",
      module: import.meta.url,
      func: "sleep",
    },
  };

  // createHandlerWithConfig wraps as: All(Identity, Constant(config)) → Invoke
  // We replicate that structure here.
  const action = typedAction<any, void>({
    kind: "Chain",
    first: {
      kind: "All",
      actions: [
        { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Identity" } } },
        { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Constant", value: config } } },
      ],
    },
    rest: invokeAction,
  });

  // Attach __definition for the worker (non-enumerable, invisible to JSON).
  const definition = {
    handle: ({ value }: { value: unknown }) => {
      const [, stepConfig] = value as [unknown, { ms: number }];
      return new Promise<void>((resolve) => setTimeout(resolve, stepConfig.ms));
    },
  };

  Object.defineProperty(action, "__definition", {
    value: definition,
    enumerable: false,
  });

  return action;
}

// Also attach __definition on the sleep function itself, since the worker
// imports the module export and accesses __definition from it.
Object.defineProperty(sleep, "__definition", {
  value: {
    handle: ({ value }: { value: unknown }) => {
      const [, stepConfig] = value as [unknown, { ms: number }];
      return new Promise<void>((resolve) => setTimeout(resolve, stepConfig.ms));
    },
  },
  enumerable: false,
});

// ---------------------------------------------------------------------------
// withTimeout — race body against sleep, return Result
// ---------------------------------------------------------------------------

/**
 * Race the body against a sleep timer. Returns `Result<TOut, void>`:
 * - Ok(value) if the body completed first
 * - Err(void) if the timeout fired first
 *
 * Built as raw AST rather than through `race()` because each branch wraps
 * its result differently (Ok vs Err) before Perform. `race()` requires
 * homogeneous output types, but withTimeout needs heterogeneous tagging.
 *
 * Compiles to the same Handle/All/Perform structure as race, with each
 * branch wrapping its result as Ok or Err before Perform.
 */
export function withTimeout<TIn, TOut>(
  ms: number,
  body: Pipeable<TIn, TOut>,
): TypedAction<TIn, Result<TOut, void>> {
  const effectId = allocateEffectId();

  const perform: Action = { kind: "Perform", effect_id: effectId };

  // Branch 1: body → Tag("Ok") → Perform
  const bodyBranch: Action = {
    kind: "Chain",
    first: { kind: "Chain", first: body as Action, rest: TAG_OK },
    rest: perform,
  };

  // Branch 2: sleep → Tag("Err") → Perform
  const sleepBranch: Action = {
    kind: "Chain",
    first: { kind: "Chain", first: sleep({ ms }) as Action, rest: TAG_ERR },
    rest: perform,
  };

  return typedAction({
    kind: "Handle",
    effect_id: effectId,
    handler: RACE_HANDLER,
    body: { kind: "All", actions: [bodyBranch, sleepBranch] },
  });
}

// ---------------------------------------------------------------------------
// invokeWithTimeout — withTimeout + invokeWithThrow
// ---------------------------------------------------------------------------

/**
 * Run a fallible handler with a timeout. On timeout or handler error, throw.
 * Combines `withTimeout` + `invokeWithThrow`.
 *
 * The handler returns `Result<TOut, TError>`. After withTimeout, we have
 * `Result<Result<TOut, TError>, void>`:
 * - Ok(Result<TOut, TError>): handler completed → invokeWithThrow to unwrap
 * - Err(void): timeout fired → throw void
 *
 * The throwError token must accept `TError | void` since either the handler
 * error or the timeout can trigger the throw.
 */
export function invokeWithTimeout<TIn, TOut, TError>(
  handler: Pipeable<TIn, Result<TOut, TError>>,
  ms: number,
  throwError: Pipeable<TError | void, never>,
): TypedAction<TIn, TOut> {
  // withTimeout(ms, handler) → Result<Result<TOut, TError>, void>
  // Branch on outer Result:
  //   Ok: receives Result<TOut, TError> → invokeWithThrow → TOut or throw TError
  //   Err: receives void → throwError
  return typedAction({
    kind: "Chain",
    first: withTimeout(ms, handler) as Action,
    rest: {
      kind: "Branch",
      cases: {
        Ok: {
          kind: "Chain",
          first: EXTRACT_VALUE,
          rest: invokeWithThrow(identity() as Pipeable<Result<TOut, TError>, Result<TOut, TError>>, throwError as Pipeable<TError, never>) as Action,
        },
        Err: {
          kind: "Chain",
          first: EXTRACT_VALUE,
          rest: throwError as Action,
        },
      },
    },
  });
}
