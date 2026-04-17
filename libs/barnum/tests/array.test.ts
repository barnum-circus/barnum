import { describe, it, expect } from "vitest";
import {
  type ExtractInput,
  type ExtractOutput,
  type Option,
  pipe,
  forEach,
} from "../src/ast.js";
import {
  constant,
  getIndex,
  flatten,
  splitFirst,
  splitLast,
  range,
} from "../src/builtins/index.js";
import { first, last } from "../src/option.js";
import { runPipeline } from "../src/run.js";
import { verify } from "./handlers.js";

// ---------------------------------------------------------------------------
// Type assertion helpers (compile-time only)
// ---------------------------------------------------------------------------

type IsExact<T, U> = [T] extends [U] ? ([U] extends [T] ? true : false) : false;

// eslint-disable-next-line @typescript-eslint/no-unused-vars
function assertExact<_T extends true>(): void {}

// ---------------------------------------------------------------------------
// Type tests
// ---------------------------------------------------------------------------

describe("array type tests", () => {
  it("range: any -> number[]", () => {
    const action = range(0, 10);
    assertExact<IsExact<ExtractInput<typeof action>, any>>();
    assertExact<IsExact<ExtractOutput<typeof action>, number[]>>();
    expect(action.kind).toBe("Invoke");
  });

  it("flatten: T[][] -> T[]", () => {
    const action = flatten<number>();
    assertExact<IsExact<ExtractInput<typeof action>, number[][]>>();
    assertExact<IsExact<ExtractOutput<typeof action>, number[]>>();
    expect(action.kind).toBe("Invoke");
  });

  it("getIndex: Tuple -> Option<Tuple[N]>", () => {
    const action = getIndex<[string, number, boolean], 1>(1);
    assertExact<IsExact<ExtractInput<typeof action>, [string, number, boolean]>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Option<number>>>();
    expect(action.kind).toBe("Invoke");
  });

  it("splitFirst: T[] -> Option<[T, T[]]>", () => {
    const action = splitFirst<number>();
    assertExact<IsExact<ExtractInput<typeof action>, number[]>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Option<[number, number[]]>>>();
    expect(action.kind).toBe("Invoke");
  });

  it("splitLast: T[] -> Option<[T[], T]>", () => {
    const action = splitLast<number>();
    assertExact<IsExact<ExtractInput<typeof action>, number[]>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Option<[number[], number]>>>();
    expect(action.kind).toBe("Invoke");
  });

  it("first: T[] -> Option<T>", () => {
    const action = first<number>();
    assertExact<IsExact<ExtractInput<typeof action>, readonly number[]>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Option<number>>>();
  });

  it("last: T[] -> Option<T>", () => {
    const action = last<number>();
    assertExact<IsExact<ExtractInput<typeof action>, readonly number[]>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Option<number>>>();
  });
});

// ---------------------------------------------------------------------------
// AST structure tests
// ---------------------------------------------------------------------------

describe("array AST structure", () => {
  it(".flatten() produces Chain -> Flatten AST", () => {
    const action = forEach(forEach(verify)).flatten();
    expect(action.kind).toBe("Chain");
    const chain = action as { kind: "Chain"; first: any; rest: any };
    expect(chain.first.kind).toBe("ForEach");
    expect(chain.rest.kind).toBe("Invoke");
    expect(chain.rest.handler.builtin.kind).toBe("Flatten");
  });
});

// ---------------------------------------------------------------------------
// Execution tests
// ---------------------------------------------------------------------------

describe("array execution", () => {
  // -- range --
  it("range(0, 5) returns [0, 1, 2, 3, 4]", async () => {
    const result = await runPipeline(range(0, 5));
    expect(result).toEqual([0, 1, 2, 3, 4]);
  });

  it("range(3, 3) returns []", async () => {
    const result = await runPipeline(range(3, 3));
    expect(result).toEqual([]);
  });

  it("range(2, 5) returns [2, 3, 4]", async () => {
    const result = await runPipeline(range(2, 5));
    expect(result).toEqual([2, 3, 4]);
  });

  // -- flatten --
  it("flatten([[1, 2], [3]]) -> [1, 2, 3]", async () => {
    const result = await runPipeline(
      pipe(constant([[1, 2], [3]]), flatten()),
    );
    expect(result).toEqual([1, 2, 3]);
  });

  it("flatten([]) -> []", async () => {
    const result = await runPipeline(
      pipe(constant([] as number[][]), flatten()),
    );
    expect(result).toEqual([]);
  });

  it("flatten([[], [1], []]) -> [1]", async () => {
    const result = await runPipeline(
      pipe(constant([[], [1], []]), flatten()),
    );
    expect(result).toEqual([1]);
  });

  // -- getIndex --
  it("getIndex(0) on [10, 20, 30] -> Some(10)", async () => {
    const result = await runPipeline(
      pipe(constant([10, 20, 30]), getIndex(0)),
    );
    expect(result).toEqual({ kind: "Option.Some", value: 10 });
  });

  it("getIndex(2) on [10, 20, 30] -> Some(30)", async () => {
    const result = await runPipeline(
      pipe(constant([10, 20, 30]), getIndex(2)),
    );
    expect(result).toEqual({ kind: "Option.Some", value: 30 });
  });

  it("getIndex(5) on [10, 20, 30] -> None", async () => {
    const result = await runPipeline(
      pipe(constant([10, 20, 30]), getIndex(5)),
    );
    expect(result).toEqual({ kind: "Option.None", value: null });
  });

  it("getIndex(0) on [] -> None", async () => {
    const result = await runPipeline(
      pipe(constant([] as number[]), getIndex(0)),
    );
    expect(result).toEqual({ kind: "Option.None", value: null });
  });

  // -- splitFirst --
  it("splitFirst on [1, 2, 3] -> Some([1, [2, 3]])", async () => {
    const result = await runPipeline(
      pipe(constant([1, 2, 3]), splitFirst()),
    );
    expect(result).toEqual({ kind: "Option.Some", value: [1, [2, 3]] });
  });

  it("splitFirst on [42] -> Some([42, []])", async () => {
    const result = await runPipeline(
      pipe(constant([42]), splitFirst()),
    );
    expect(result).toEqual({ kind: "Option.Some", value: [42, []] });
  });

  it("splitFirst on [] -> None", async () => {
    const result = await runPipeline(
      pipe(constant([] as number[]), splitFirst()),
    );
    expect(result).toEqual({ kind: "Option.None", value: null });
  });

  // -- splitLast --
  it("splitLast on [1, 2, 3] -> Some([[1, 2], 3])", async () => {
    const result = await runPipeline(
      pipe(constant([1, 2, 3]), splitLast()),
    );
    expect(result).toEqual({ kind: "Option.Some", value: [[1, 2], 3] });
  });

  it("splitLast on [42] -> Some([[], 42])", async () => {
    const result = await runPipeline(
      pipe(constant([42]), splitLast()),
    );
    expect(result).toEqual({ kind: "Option.Some", value: [[], 42] });
  });

  it("splitLast on [] -> None", async () => {
    const result = await runPipeline(
      pipe(constant([] as number[]), splitLast()),
    );
    expect(result).toEqual({ kind: "Option.None", value: null });
  });

  // -- first --
  it("first on [10, 20] -> Some(10)", async () => {
    const result = await runPipeline(
      pipe(constant([10, 20]), first()),
    );
    expect(result).toEqual({ kind: "Option.Some", value: 10 });
  });

  it("first on [] -> None", async () => {
    const result = await runPipeline(
      pipe(constant([] as number[]), first()),
    );
    expect(result).toEqual({ kind: "Option.None", value: null });
  });

  // -- last --
  it("last on [10, 20] -> Some(20)", async () => {
    const result = await runPipeline(
      pipe(constant([10, 20]), last()),
    );
    expect(result).toEqual({ kind: "Option.Some", value: 20 });
  });

  it("last on [] -> None", async () => {
    const result = await runPipeline(
      pipe(constant([] as number[]), last()),
    );
    expect(result).toEqual({ kind: "Option.None", value: null });
  });
});
