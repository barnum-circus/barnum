import type { Pipeable, Result, TypedAction } from "./ast.js";
import { bindInput } from "./bind.js";

/**
 * Retry an action up to `maxAttempts` times. The action must return a Result.
 * On first Ok, returns immediately. If all attempts produce Err, returns the
 * final Err.
 *
 * Compiles to static `.or()` unrolling: `action.or(action).or(action)...`
 * The retry count is baked into the AST at construction time.
 */
export function withRetry<TIn, TOk, TErr>(
  action: Pipeable<TIn, Result<TOk, TErr>>,
  maxAttempts: number,
): TypedAction<TIn, Result<TOk, TErr>> {
  return bindInput<TIn, Result<TOk, TErr>>((input) => {
    let pipeline = input.then(action);
    for (let i = 1; i < maxAttempts; i++) {
      pipeline = pipeline.or(input.then(action));
    }
    return pipeline;
  });
}
