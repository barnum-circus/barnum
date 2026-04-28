/**
 * withRetry — higher-order combinator that retries a fallible action
 * up to 3 times before giving up.
 *
 * The action must return Result<TOut, string>. On Ok, the value is
 * unwrapped. On Err, the action is retried with the original input.
 * After all attempts are exhausted, the pipeline panics.
 *
 * Uses bindInput to capture the original input, then nested tryCatch:
 * each level tries the action and falls through to the next on failure.
 */

import type { Pipeable, TypedAction, Result } from "@barnum/barnum/pipeline";
import { bindInput, tryCatch, drop, panic } from "@barnum/barnum/pipeline";

export function withRetry<TIn, TOut>(
  _maxAttempts: 3,
  action: Pipeable<TIn, Result<TOut, string>>,
): TypedAction<TIn, TOut> {
  return bindInput<TIn, TOut>((originalInput) =>
    tryCatch(
      (throw1: TypedAction<string, never>) =>
        originalInput.then(action).unwrapOr(throw1),
      drop.then(
        tryCatch(
          (throw2: TypedAction<string, never>) =>
            originalInput.then(action).unwrapOr(throw2),
          drop.then(
            tryCatch(
              (throw3: TypedAction<string, never>) =>
                originalInput.then(action).unwrapOr(throw3),
              drop.then(panic("max retries (3) exceeded")),
            ),
          ),
        ),
      ),
    ),
  );
}
