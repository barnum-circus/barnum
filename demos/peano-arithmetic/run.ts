/**
 * Peano arithmetic: is-even / is-odd via mutual recursion.
 *
 * isEven(0) = true       isOdd(0) = false
 * isEven(n) = isOdd(n-1) isOdd(n) = isEven(n-1)
 *
 * isEven(7) → isOdd(6) → isEven(5) → isOdd(4)
 *           → isEven(3) → isOdd(2) → isEven(1) → isOdd(0) → false
 */

import {
  pipe,
  constant,
  runPipeline,
  defineRecursiveFunctions,
} from "@barnum/barnum";
import { classifyZero, subtractOne } from "./handlers/steps.js";

runPipeline(
  defineRecursiveFunctions<[
    [number, boolean], // isEven: number → boolean
    [number, boolean], // isOdd:  number → boolean
  ]>(
    (isEven, isOdd) => [
      // isEven body
      classifyZero.branch({
        Zero: constant(true),
        NonZero: pipe(subtractOne, isOdd),
      }),
      // isOdd body
      classifyZero.branch({
        Zero: constant(false),
        NonZero: pipe(subtractOne, isEven),
      }),
    ],
  )((isEven, _isOdd) => isEven),
  7,
);
