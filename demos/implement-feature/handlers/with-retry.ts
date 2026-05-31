/**
 * withRetry — higher-order combinator that retries a fallible action
 * up to N times before giving up.
 *
 * The action must return Result<TOut, string>. On Ok, the value is
 * unwrapped and the loop exits via earlyReturn. On Err, checkRetries
 * decrements the remaining count: Retry → loop again, Exhausted → panic.
 *
 * Uses bindInput to capture the original input, loop to track retry
 * state, and tryCatch to handle each attempt.
 */

import type { Pipeable, TypedAction, Result } from "@barnum/barnum/pipeline";
import {
  bindInput,
  typed,
  pipe,
  tryCatch,
  loop,
  earlyReturn,
  drop,
  panic,
  constant,
} from "@barnum/barnum/pipeline";
import { checkRetries } from "./steps";

export function withRetry<TIn, TOut>(
  maxAttempts: number,
  action: Pipeable<TIn, Result<TOut, string>>,
): TypedAction<TIn, TOut> {
  return bindInput<TIn, TOut>((originalInput) =>
    earlyReturn<TOut>((ret) => {
      const retryLoop = loop<number, void>((recur, _done) =>
        bindInput<number, never>((retriesRemaining) =>
          tryCatch(
            (throwError: TypedAction<string, never>) => {
              const result = typed(action).call(originalInput);
              const unwrapped = result.unwrapOr(throwError);
              return ret.call(unwrapped);
            },
            pipe(
              drop,
              checkRetries.call(retriesRemaining).branch({
                Retry: recur,
                Exhausted: pipe(drop, panic("max retries exceeded")),
              }),
            ),
          ),
        ),
      );
      return retryLoop.call(constant(maxAttempts - 1));
    }),
  );
}
