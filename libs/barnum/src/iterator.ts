import {
  type Iterator as IteratorT,
  type IteratorDef,
  type Option as OptionT,
  type Pipeable,
  type Result as ResultT,
  type TypedAction,
  toAction,
  branch,
  branchFamily,
  forEach,
} from "./ast.js";
import { chain } from "./chain.js";
import {
  constant,
  drop,
  flatten,
  getField,
  identity,
  tag,
} from "./builtins/index.js";
import { all } from "./all.js";
import { Option } from "./option.js";
import { bindInput } from "./bind.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/**
 * Wrap a single value in an array. `T → T[]`
 * Implemented as `all(identity())`. May warrant a dedicated builtin later.
 */
function wrapInArray<TElement>(): TypedAction<TElement, TElement[]> {
  return all(identity()) as TypedAction<TElement, TElement[]>;
}

/**
 * Normalize any IntoIterator return type to a plain array.
 * Used inside `.flatMap()` to handle Iterator, Option, Result, and Array returns.
 */
const intoIteratorNormalize = branchFamily({
  Iterator: branch({ Iterator: identity() }),
  Option: branch({ Some: wrapInArray(), None: constant([]) }),
  Result: branch({ Ok: wrapInArray(), Err: constant([]) }),
  Array: identity(),
});

// ---------------------------------------------------------------------------
// Iterator namespace
// ---------------------------------------------------------------------------

export const Iterator = {
  /** Wrap an array as Iterator. `T[] → Iterator<T>` */
  fromArray<TElement>(): TypedAction<TElement[], IteratorT<TElement>> {
    return tag<"Iterator", IteratorDef<TElement>, "Iterator">("Iterator", "Iterator");
  },

  /** Wrap an Option as Iterator. `Option<T> → Iterator<T>` */
  fromOption<TElement>(): TypedAction<OptionT<TElement>, IteratorT<TElement>> {
    return chain(
      toAction(branch({
        Some: chain(toAction(wrapInArray<TElement>()), toAction(Iterator.fromArray<TElement>())),
        None: chain(toAction(constant([] as TElement[])), toAction(Iterator.fromArray<TElement>())),
      })),
      toAction(identity()),
    ) as TypedAction<OptionT<TElement>, IteratorT<TElement>>;
  },

  /** Wrap a Result as Iterator (Ok kept, Err dropped). `Result<T, E> → Iterator<T>` */
  fromResult<TElement, TError>(): TypedAction<ResultT<TElement, TError>, IteratorT<TElement>> {
    return chain(
      toAction(branch({
        Ok: chain(toAction(wrapInArray<TElement>()), toAction(Iterator.fromArray<TElement>())),
        Err: chain(toAction(constant([] as TElement[])), toAction(Iterator.fromArray<TElement>())),
      })),
      toAction(identity()),
    ) as TypedAction<ResultT<TElement, TError>, IteratorT<TElement>>;
  },

  /** Unwrap Iterator to array. `Iterator<T> → T[]` */
  collect<TElement>(): TypedAction<IteratorT<TElement>, TElement[]> {
    return getField("value") as TypedAction<IteratorT<TElement>, TElement[]>;
  },

  /** Transform each element. `Iterator<T> → Iterator<U>` */
  map<TIn, TOut>(action: Pipeable<TIn, TOut>): TypedAction<IteratorT<TIn>, IteratorT<TOut>> {
    return chain(
      toAction(getField("value")),
      chain(
        toAction(forEach(action)),
        toAction(Iterator.fromArray<TOut>()),
      ),
    ) as TypedAction<IteratorT<TIn>, IteratorT<TOut>>;
  },

  /** Flat-map each element. `f` returns any IntoIterator type. `Iterator<T> → Iterator<U>` */
  flatMap<TIn, TOut>(
    action: Pipeable<TIn, unknown>,
  ): TypedAction<IteratorT<TIn>, IteratorT<TOut>> {
    return chain(
      toAction(getField("value")),
      chain(
        toAction(forEach(chain(toAction(action), toAction(intoIteratorNormalize)))),
        chain(
          toAction(flatten()),
          toAction(Iterator.fromArray<TOut>()),
        ),
      ),
    ) as TypedAction<IteratorT<TIn>, IteratorT<TOut>>;
  },

  /** Keep elements where predicate returns true. `Iterator<T> → Iterator<T>` */
  filter<TElement>(
    predicate: Pipeable<TElement, boolean>,
  ): TypedAction<IteratorT<TElement>, IteratorT<TElement>> {
    return Iterator.flatMap<TElement, TElement>(
      bindInput<TElement>((element) =>
        element.then(predicate).asOption().branch({
          Some: element.some(),
          None: chain(toAction(drop), toAction(Option.none<TElement>())),
        }),
      ),
    );
  },
} as const;
