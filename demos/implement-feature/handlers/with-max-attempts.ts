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
  return earlyReturn<TBreak, null, never>((ret) =>
    constant(maxAttempts - 1).then(
      loop<number, never>((recur, _done) =>
        bindInput<number, never>((attemptsRemaining) => {
          const guardedRecur: TypedAction<null, never> = attemptsRemaining
            .then(checkRetries)
            .branch({
              Retry: recur,
              Exhausted: drop.then(panic("max review attempts exceeded")),
            });
          return drop.then(bodyFn(guardedRecur, ret));
        }),
      ),
    ),
  );
}
