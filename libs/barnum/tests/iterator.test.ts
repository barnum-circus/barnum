import { describe, expect, it } from "vitest";
import {
  type Iterator,
  type Option,
  type OptionDef,
  type Result,
  type ResultDef,
  bindInput,
  pipe,
} from "../src/ast.js";
import { constant, identity, tag } from "../src/builtins/index.js";
import { Iterator as I } from "../src/iterator.js";
import { Option as O } from "../src/option.js";
import { runPipeline } from "../src/run.js";
import { type CheckIO, assertExact } from "./type-utils.js";

// ---------------------------------------------------------------------------
// Type tests — standalone constructors
// ---------------------------------------------------------------------------

describe("Iterator constructor type info", () => {
  it("Iterator.fromArray<T>(): T[] → Iterator<T>", () => {
    const action = I.fromArray<number>();
    assertExact<CheckIO<typeof action, Array<number>, Iterator<number>>>();
  });

  it("Iterator.fromOption<T>(): Option<T> → Iterator<T>", () => {
    const action = I.fromOption<string>();
    assertExact<CheckIO<typeof action, Option<string>, Iterator<string>>>();
  });

  it("Iterator.fromResult<T,E>(): Result<T,E> → Iterator<T>", () => {
    const action = I.fromResult<number, string>();
    assertExact<
      CheckIO<typeof action, Result<number, string>, Iterator<number>>
    >();
  });

  it("Iterator.collect<T>(): Iterator<T> → T[]", () => {
    const action = I.collect<number>();
    assertExact<CheckIO<typeof action, Iterator<number>, Array<number>>>();
  });

  it("Iterator.map<T,U>(f): Iterator<T> → Iterator<U>", () => {
    const action = I.map<number, string>(constant("x"));
    assertExact<CheckIO<typeof action, Iterator<number>, Iterator<string>>>();
  });

  it("Iterator.flatMap<T,U>(f): Iterator<T> → Iterator<U>", () => {
    const action = I.flatMap<number, string>(constant(["a", "b"]));
    assertExact<CheckIO<typeof action, Iterator<number>, Iterator<string>>>();
  });

  it("Iterator.filter<T>(pred): Iterator<T> → Iterator<T>", () => {
    const action = I.filter<number>(constant(true));
    assertExact<CheckIO<typeof action, Iterator<number>, Iterator<number>>>();
  });
});

// ---------------------------------------------------------------------------
// Type tests — postfix methods
// ---------------------------------------------------------------------------

describe("Iterator postfix type info", () => {
  it(".iterate() on Option", () => {
    const action = constant(42).some().iterate();
    assertExact<CheckIO<typeof action, any, Iterator<number>>>();
  });

  it(".iterate() on Result", () => {
    const action = constant(42).ok().iterate();
    assertExact<CheckIO<typeof action, any, Iterator<number>>>();
  });

  it(".iterate() on array", () => {
    const action = constant([1, 2, 3]).iterate();
    assertExact<CheckIO<typeof action, any, Iterator<number>>>();
  });

  it(".map(f) on Iterator", () => {
    const action = constant([1, 2]).iterate().map(constant("x"));
    assertExact<CheckIO<typeof action, any, Iterator<string>>>();
  });

  it(".flatMap(f) on Iterator returning Iterator", () => {
    const action = constant([1])
      .iterate()
      .flatMap(pipe(constant([10, 20]), I.fromArray<number>()));
    assertExact<CheckIO<typeof action, any, Iterator<number>>>();
  });

  it(".flatMap(f) on Iterator returning Option", () => {
    const action = constant([1]).iterate().flatMap(constant(42).some());
    assertExact<CheckIO<typeof action, any, Iterator<number>>>();
  });

  it(".flatMap(f) on Iterator returning Result", () => {
    const action = constant([1]).iterate().flatMap(constant(42).ok());
    assertExact<CheckIO<typeof action, any, Iterator<number>>>();
  });

  it(".flatMap(f) on Iterator returning array", () => {
    const action = constant([1])
      .iterate()
      .flatMap(constant([10, 20]));
    assertExact<CheckIO<typeof action, any, Iterator<number>>>();
  });

  it(".filter(pred) on Iterator", () => {
    const action = constant([1, 2, 3]).iterate().filter(constant(true));
    assertExact<CheckIO<typeof action, any, Iterator<number>>>();
  });

  it(".collect() on Iterator", () => {
    const action = constant([1, 2]).iterate().collect();
    assertExact<CheckIO<typeof action, any, Array<number>>>();
  });

  it(".fold() types: input and output", () => {
    const action = constant([1, 2, 3])
      .iterate()
      .fold(
        constant(""),
        bindInput<[string, number], string>((state) => {
          const [acc, element] = state.split();
          assertExact<CheckIO<typeof acc, any, string>>();
          assertExact<CheckIO<typeof element, any, number>>();
          return acc;
        }),
      );
    assertExact<CheckIO<typeof action, any, string>>();
  });

  it(".isEmpty() on Iterator", () => {
    const action = constant([1, 2]).iterate().isEmpty();
    assertExact<CheckIO<typeof action, any, boolean>>();
  });

  it(".take(n) on Iterator", () => {
    const action = constant([1, 2, 3]).iterate().take(2);
    assertExact<CheckIO<typeof action, any, Iterator<number>>>();
  });

  it(".skip(n) on Iterator", () => {
    const action = constant([1, 2, 3]).iterate().skip(1);
    assertExact<CheckIO<typeof action, any, Iterator<number>>>();
  });

  it(".splitFirst() on Iterator", () => {
    const action = constant([1, 2, 3]).iterate().splitFirst();
    assertExact<
      CheckIO<typeof action, any, Option<[number, Iterator<number>]>>
    >();
  });

  it(".splitLast() on Iterator", () => {
    const action = constant([1, 2, 3]).iterate().splitLast();
    assertExact<
      CheckIO<typeof action, any, Option<[Iterator<number>, number]>>
    >();
  });

  it("full chain: array.iterate().map(f).filter(p).collect()", () => {
    const action = constant([1, 2, 3])
      .iterate()
      .map(constant("x"))
      .filter(constant(true))
      .collect();
    assertExact<CheckIO<typeof action, any, Array<string>>>();
  });
});

// ---------------------------------------------------------------------------
// Type tests — standalone fold
// ---------------------------------------------------------------------------

describe("Iterator.fold type info", () => {
  it("fold infers input from Iterator, output from accumulator", () => {
    const action = I.fold<number, string>(
      constant("init"),
      bindInput<[string, number], string>((state) => {
        const [acc, element] = state.split();
        assertExact<CheckIO<typeof acc, any, string>>();
        assertExact<CheckIO<typeof element, any, number>>();
        return acc;
      }),
    );
    assertExact<CheckIO<typeof action, Iterator<number>, string>>();
  });
});

// ---------------------------------------------------------------------------
// Execution tests
// ---------------------------------------------------------------------------

describe("Iterator execution", () => {
  // -- fromArray / collect round-trip --
  it("fromArray wraps array", async () => {
    const result = await runPipeline(pipe(constant([1, 2, 3]), I.fromArray()));
    expect(result).toEqual({ kind: "Iterator.Iterator", value: [1, 2, 3] });
  });

  it("collect unwraps Iterator", async () => {
    const result = await runPipeline(
      pipe(constant([1, 2, 3]), I.fromArray(), I.collect()),
    );
    expect(result).toEqual([1, 2, 3]);
  });

  it("round-trip: fromArray → collect", async () => {
    const result = await runPipeline(
      pipe(constant([10, 20, 30]), I.fromArray<number>(), I.collect<number>()),
    );
    expect(result).toEqual([10, 20, 30]);
  });

  // -- fromOption --
  it("fromOption on Some → single-element Iterator", async () => {
    const result = await runPipeline(
      pipe(constant(42).some(), I.fromOption<number>(), I.collect<number>()),
    );
    expect(result).toEqual([42]);
  });

  it("fromOption on None → empty Iterator", async () => {
    const result = await runPipeline(
      pipe(
        pipe(constant(null), O.none<number>()),
        I.fromOption<number>(),
        I.collect<number>(),
      ),
    );
    expect(result).toEqual([]);
  });

  // -- fromResult --
  it("fromResult on Ok → single-element Iterator", async () => {
    const result = await runPipeline(
      pipe(
        constant(42).ok(),
        I.fromResult<number, string>(),
        I.collect<number>(),
      ),
    );
    expect(result).toEqual([42]);
  });

  it("fromResult on Err → empty Iterator", async () => {
    const result = await runPipeline(
      pipe(
        constant("oops").err(),
        I.fromResult<number, string>(),
        I.collect<number>(),
      ),
    );
    expect(result).toEqual([]);
  });

  // -- map --
  it("Iterator.map transforms each element", async () => {
    const result = await runPipeline(
      pipe(
        constant([1, 2, 3]),
        I.fromArray<number>(),
        I.map<number, number>(constant(99)),
        I.collect<number>(),
      ),
    );
    expect(result).toEqual([99, 99, 99]);
  });

  // -- flatMap with Iterator return --
  it("flatMap where f returns Iterator", async () => {
    const result = await runPipeline(
      pipe(
        constant([1, 2]),
        I.fromArray<number>(),
        I.flatMap<number, number>(
          pipe(constant([10, 20]), I.fromArray<number>()),
        ),
        I.collect<number>(),
      ),
    );
    expect(result).toEqual([10, 20, 10, 20]);
  });

  // -- flatMap with Option return --
  it("flatMap where f returns Option: Some kept, None dropped", async () => {
    const result = await runPipeline(
      pipe(
        constant([1, 2, 3]),
        I.fromArray<number>(),
        // Even numbers → Some, odd → None
        I.flatMap<number, number>(
          pipe(
            identity<number>(),
            tag<"Option", OptionDef<number>, "Some">("Some", "Option"),
          ),
        ),
        I.collect<number>(),
      ),
    );
    // All wrapped as Some since our mock always returns Some
    expect(result).toEqual([1, 2, 3]);
  });

  // -- flatMap with Result return --
  it("flatMap where f returns Result: Ok kept, Err dropped", async () => {
    const result = await runPipeline(
      pipe(
        constant([1, 2]),
        I.fromArray<number>(),
        I.flatMap<number, number>(
          pipe(
            identity<number>(),
            tag<"Result", ResultDef<number, string>, "Ok">("Ok", "Result"),
          ),
        ),
        I.collect<number>(),
      ),
    );
    expect(result).toEqual([1, 2]);
  });

  // -- flatMap with array return --
  it("flatMap where f returns array: concatenated", async () => {
    const result = await runPipeline(
      pipe(
        constant([1, 2]),
        I.fromArray<number>(),
        I.flatMap<number, string>(constant(["a", "b"])),
        I.collect<string>(),
      ),
    );
    expect(result).toEqual(["a", "b", "a", "b"]);
  });

  // -- filter --
  it("filter keeps elements where predicate is true", async () => {
    const result = await runPipeline(
      pipe(
        constant([1, 2, 3]),
        I.fromArray<number>(),
        I.filter<number>(constant(true)),
        I.collect<number>(),
      ),
    );
    expect(result).toEqual([1, 2, 3]);
  });

  it("filter drops elements where predicate is false", async () => {
    const result = await runPipeline(
      pipe(
        constant([1, 2, 3]),
        I.fromArray<number>(),
        I.filter<number>(constant(false)),
        I.collect<number>(),
      ),
    );
    expect(result).toEqual([]);
  });

  // -- postfix .iterate() --
  it(".iterate() on Some → single-element Iterator", async () => {
    const result = await runPipeline(constant(42).some().iterate().collect());
    expect(result).toEqual([42]);
  });

  it(".iterate() on None → empty Iterator", async () => {
    const result = await runPipeline(
      pipe(constant(null), O.none<number>()).iterate().collect(),
    );
    expect(result).toEqual([]);
  });

  it(".iterate() on Ok → single-element Iterator", async () => {
    const result = await runPipeline(constant(42).ok().iterate().collect());
    expect(result).toEqual([42]);
  });

  it(".iterate() on Err → empty Iterator", async () => {
    const result = await runPipeline(
      constant("oops").err().iterate().collect(),
    );
    expect(result).toEqual([]);
  });

  it(".iterate() on array → Iterator wrapping array", async () => {
    const result = await runPipeline(constant([1, 2, 3]).iterate().collect());
    expect(result).toEqual([1, 2, 3]);
  });

  // -- postfix chains --
  it("postfix: array.iterate().map(f).collect()", async () => {
    const result = await runPipeline(
      constant([1, 2, 3]).iterate().map(constant(0)).collect(),
    );
    expect(result).toEqual([0, 0, 0]);
  });

  it("postfix: array.iterate().filter(pred).collect()", async () => {
    const result = await runPipeline(
      constant([1, 2, 3]).iterate().filter(constant(true)).collect(),
    );
    expect(result).toEqual([1, 2, 3]);
  });

  it("postfix: array.iterate().flatMap(f_returning_option).collect()", async () => {
    const result = await runPipeline(
      constant([1, 2, 3])
        .iterate()
        .flatMap(
          pipe(
            identity<number>(),
            tag<"Option", OptionDef<number>, "Some">("Some", "Option"),
          ),
        )
        .collect(),
    );
    expect(result).toEqual([1, 2, 3]);
  });

  it("postfix: option.iterate().map(f).collect()", async () => {
    const result = await runPipeline(
      constant(42).some().iterate().map(constant(99)).collect(),
    );
    expect(result).toEqual([99]);
  });

  // -- empty Iterator operations --
  it("map on empty Iterator → empty", async () => {
    const result = await runPipeline(
      pipe(
        constant([] as Array<number>),
        I.fromArray<number>(),
        I.map<number, string>(constant("x")),
        I.collect<string>(),
      ),
    );
    expect(result).toEqual([]);
  });

  it("filter on empty Iterator → empty", async () => {
    const result = await runPipeline(
      pipe(
        constant([] as Array<number>),
        I.fromArray<number>(),
        I.filter<number>(constant(true)),
        I.collect<number>(),
      ),
    );
    expect(result).toEqual([]);
  });

  it("flatMap on empty Iterator → empty", async () => {
    const result = await runPipeline(
      pipe(
        constant([] as Array<number>),
        I.fromArray<number>(),
        I.flatMap<number, string>(constant(["a"])),
        I.collect<string>(),
      ),
    );
    expect(result).toEqual([]);
  });
});
