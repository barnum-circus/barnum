import {
  type Iterator as IteratorT,
  type IteratorDef,
  type Option as OptionT,
  type Pipeable,
  type Result as ResultT,
  type TypedAction,
  toAction,
  typedAction,
  branch,
  branchFamily,
  forEach,
  loop,
} from "./ast.js";
import { chain } from "./chain.js";
import {
  constant,
  drop,
  flatten,
  getField,
  getIndex,
  identity,
  splitFirst,
  splitLast,
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
    return tag<"Iterator", IteratorDef<TElement>, "Iterator">(
      "Iterator",
      "Iterator",
    );
  },

  /** Wrap an Option as Iterator. `Option<T> → Iterator<T>` */
  fromOption<TElement>(): TypedAction<OptionT<TElement>, IteratorT<TElement>> {
    return branch({
      Some: chain(wrapInArray<TElement>(), Iterator.fromArray<TElement>()),
      None: chain(constant<TElement[]>([]), Iterator.fromArray<TElement>()),
    }) as TypedAction<OptionT<TElement>, IteratorT<TElement>>;
  },

  /** Wrap a Result as Iterator (Ok kept, Err dropped). `Result<T, E> → Iterator<T>` */
  fromResult<TElement, TError>(): TypedAction<
    ResultT<TElement, TError>,
    IteratorT<TElement>
  > {
    return branch({
      Ok: chain(wrapInArray<TElement>(), Iterator.fromArray<TElement>()),
      Err: chain(constant<TElement[]>([]), Iterator.fromArray<TElement>()),
    }) as TypedAction<ResultT<TElement, TError>, IteratorT<TElement>>;
  },

  /** Unwrap Iterator to array. `Iterator<T> → T[]` */
  collect<TElement>(): TypedAction<IteratorT<TElement>, TElement[]> {
    return getField("value") as TypedAction<IteratorT<TElement>, TElement[]>;
  },

  /** Transform each element. `Iterator<T> → Iterator<U>` */
  map<TIn, TOut>(
    action: Pipeable<TIn, TOut>,
  ): TypedAction<IteratorT<TIn>, IteratorT<TOut>> {
    return chain(
      toAction(getField("value")),
      chain(toAction(forEach(action)), Iterator.fromArray<TOut>()),
    ) as TypedAction<IteratorT<TIn>, IteratorT<TOut>>;
  },

  /** Flat-map each element. `f` returns any IntoIterator type. `Iterator<T> → Iterator<U>` */
  flatMap<TIn, TOut>(
    action: Pipeable<TIn, unknown>,
  ): TypedAction<IteratorT<TIn>, IteratorT<TOut>> {
    return chain(
      toAction(getField("value")),
      chain(
        toAction(forEach(chain(action, intoIteratorNormalize))),
        chain(toAction(flatten()), Iterator.fromArray<TOut>()),
      ),
    ) as TypedAction<IteratorT<TIn>, IteratorT<TOut>>;
  },

  /** Keep elements where predicate returns true. `Iterator<T> → Iterator<T>` */
  filter<TElement>(
    predicate: Pipeable<TElement, boolean>,
  ): TypedAction<IteratorT<TElement>, IteratorT<TElement>> {
    return Iterator.flatMap<TElement, TElement>(
      bindInput<TElement>((element) =>
        element
          .then(predicate)
          .asOption()
          .branch({
            Some: element.some(),
            None: chain(drop, Option.none<TElement>()),
          }),
      ),
    );
  },

  /** Head/tail decomposition. `Iterator<T> → Option<[T, Iterator<T>]>` */
  splitFirst<TElement>(): TypedAction<
    IteratorT<TElement>,
    OptionT<[TElement, IteratorT<TElement>]>
  > {
    return Iterator.collect<TElement>()
      .then(splitFirst<TElement>())
      .then(
        Option.map(
          all(
            getIndex<[TElement, TElement[]], 0>(0).unwrap(),
            getIndex<[TElement, TElement[]], 1>(1).unwrap().iterate(),
          ),
        ),
      );
  },

  /** Init/last decomposition. `Iterator<T> → Option<[Iterator<T>, T]>` */
  splitLast<TElement>(): TypedAction<
    IteratorT<TElement>,
    OptionT<[IteratorT<TElement>, TElement]>
  > {
    return Iterator.collect<TElement>()
      .then(splitLast<TElement>())
      .then(
        Option.map(
          all(
            getIndex<[TElement[], TElement], 0>(0).unwrap().iterate(),
            getIndex<[TElement[], TElement], 1>(1).unwrap(),
          ),
        ),
      );
  },

  /** Fold elements with accumulator. `Iterator<T> → TAcc` */
  fold<TElement, TAcc>(
    init: Pipeable<void, TAcc>,
    body: Pipeable<[TAcc, TElement], TAcc>,
  ): TypedAction<IteratorT<TElement>, TAcc> {
    return Iterator.collect<TElement>().then(
      bindInput<TElement[]>((elements) =>
        all(init, elements).then(
          loop<TAcc, [TAcc, TElement[]]>((recur, done) => {
            // Re-wrap done to bridge VoidToNull<TAcc> → TAcc (TypeScript
            // can't simplify the conditional type for generic TAcc).
            const doneTAcc = typedAction<TAcc, never>(toAction(done));

            // Wrap return with typedAction — branch output inference fails
            // for generic types inside loop bodies.
            return typedAction<[TAcc, TElement[]], never>(
              toAction(
                bindInput<[TAcc, TElement[]]>((state) => {
                  const acc = state.getIndex(0).unwrap();
                  const remaining = state.getIndex(1).unwrap();

                  return remaining.splitFirst().branch({
                    None: acc.then(doneTAcc),
                    Some: bindInput<[TElement, TElement[]]>((headTail) => {
                      const head = headTail.getIndex(0).unwrap();
                      const tail = headTail.getIndex(1).unwrap();

                      return all(acc, head)
                        .then(body)
                        .then(
                          bindInput<TAcc>((newAcc) =>
                            all(newAcc, tail).then(recur),
                          ),
                        );
                    }),
                  });
                }),
              ),
            );
          }),
        ),
      ),
    );
  },

  /** Check if iterator is empty. `Iterator<T> → boolean` */
  isEmpty<TElement>(): TypedAction<IteratorT<TElement>, boolean> {
    return Iterator.collect<TElement>().splitFirst().isNone();
  },
} as const;
