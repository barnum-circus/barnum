/**
 * Peano arithmetic: is-even / is-odd via mutual recursion.
 *
 * isEven(0) = true       isOdd(0) = false
 * isEven(n) = isOdd(n-1) isOdd(n) = isEven(n-1)
 *
 * isEven(7) → isOdd(6) → isEven(5) → isOdd(4)
 *           → isEven(3) → isOdd(2) → isEven(1) → isOdd(0) → false
 */

import { constant, defineRecursiveFunctions } from "@barnum/barnum/pipeline";
import { classifyZero, subtractOne } from "./handlers/steps";

defineRecursiveFunctions<
  [
    [number, boolean], // isEven: number → boolean
    [number, boolean], // isOdd:  number → boolean
  ]
>((isEven, isOdd) => [
  // isEven body
  classifyZero.branch({
    Zero: constant(true),
    NonZero: isOdd.call(subtractOne),
  }),
  // isOdd body
  classifyZero.branch({
    Zero: constant(false),
    NonZero: isEven.call(subtractOne),
  }),
])((isEven, _isOdd) => isEven)
  .call(constant(7))
  .run();
