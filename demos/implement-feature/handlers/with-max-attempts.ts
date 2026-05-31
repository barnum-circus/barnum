/**
 * withMaxAttempts — higher-order combinator that wraps a loop with
 * a maximum iteration count.
 *
 * Like loop, the body function receives (recur, done). But recur
 * is gated: after maxAttempts iterations, the pipeline panics
 * instead of looping again.
 */

import type { TypedAction, Pipeable } from "@barnum/barnum/pipeline";
import {
  loop,
  earlyReturn,
  bindInput,
  typed,
  drop,
  panic,
  constant,
} from "@barnum/barnum/pipeline";
import { checkRetries } from "./steps";

export function withMaxAttempts<TBreak>(
  maxAttempts: number,
  bodyFn: (
    recur: TypedAction<null, never>,
    done: TypedAction<TBreak, never>,
  ) => Pipeable<null, never>,
): TypedAction<null, TBreak> {
  return earlyReturn<TBreak, null, never>((ret) => {
    const retryLoop = loop<number, never>((recur, _done) =>
      bindInput<number, never>((attemptsRemaining) => {
        const guardedRecur: TypedAction<null, never> = checkRetries
          .call(attemptsRemaining)
          .branch({
            Retry: recur,
            Exhausted: panic("max review attempts exceeded").call(drop),
          });
        return typed(bodyFn(guardedRecur, ret)).call(drop);
      }),
    );
    return retryLoop.call(constant(maxAttempts - 1));
  });
}
