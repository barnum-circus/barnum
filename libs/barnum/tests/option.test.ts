import { describe, it, expect } from "vitest";
import {
  type ExtractInput,
  type ExtractOutput,
  type Option,
  type OptionDef,
  type Result,
  type ResultDef,
  type TypedAction,
  pipe,
  forEach,
} from "../src/ast.js";
import {
  constant,
  drop,
  identity,
  tag,
} from "../src/builtins/index.js";
import { Option as O } from "../src/option.js";
import { runPipeline } from "../src/run.js";
import { verify } from "./handlers.js";

// ---------------------------------------------------------------------------
// Type assertion helpers (compile-time only)
// ---------------------------------------------------------------------------

type IsExact<T, U> = [T] extends [U] ? ([U] extends [T] ? true : false) : false;

// eslint-disable-next-line @typescript-eslint/no-unused-vars
function assertExact<_T extends true>(): void {}

function expectedTagAst(kind: string) {
  return {
    kind: "Chain",
    first: {
      kind: "All",
      actions: [
        {
          kind: "Chain",
          first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Constant", value: kind } } },
          rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "WrapInField", field: "kind" } } },
        },
        { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "WrapInField", field: "value" } } },
      ],
    },
    rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Merge" } } },
  };
}

// Helpers for constructing typed Option values in execution tests
function optionSome<T>(value: T): TypedAction<any, Option<T>> {
  return pipe(constant(value), tag<"Option", OptionDef<T>, "Some">("Some", "Option"));
}
function optionNone<T>(): TypedAction<any, Option<T>> {
  return pipe(constant(null), tag<"Option", OptionDef<T>, "None">("None", "Option"));
}

// ---------------------------------------------------------------------------
// Type tests
// ---------------------------------------------------------------------------

describe("Option constructor type info", () => {
  it("Option.some() retains element type", () => {
    // @ts-expect-error — some is currently a value, not a function
    const some = O.some<number>();
    assertExact<IsExact<ExtractInput<typeof some>, number>>();
    assertExact<IsExact<ExtractOutput<typeof some>, Option<number>>>();
  });

  it("Option.none() retains element type", () => {
    // @ts-expect-error — none is currently a value, not a function
    const none = O.none<string>();
    assertExact<IsExact<ExtractOutput<typeof none>, Option<string>>>();
  });

  it("Option.some() infers type from chain context", () => {
    // @ts-expect-error — some is currently a value, not a function
    const result = constant(42).then(O.some());
    // @ts-expect-error — result is any until constructors are functions
    assertExact<IsExact<ExtractOutput<typeof result>, Option<number>>>();
  });
});

describe("Option namespace types", () => {
  it("Option.map(action): Option<T> -> Option<U>", () => {
    const action = O.map<{ artifact: string }, { verified: boolean }>(verify);
    assertExact<IsExact<ExtractInput<typeof action>, Option<{ artifact: string }>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Option<{ verified: boolean }>>>();
  });

  it("Option.map composes in pipe", () => {
    const action = pipe(
      tag<"Option", OptionDef<{ artifact: string }>, "Some">("Some", "Option"),
      O.map(verify),
    );
    assertExact<IsExact<ExtractInput<typeof action>, { artifact: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Option<{ verified: boolean }>>>();
  });

  it("Option.andThen(action): Option<T> -> Option<U>", () => {
    const action = O.andThen<{ artifact: string }, { verified: boolean }>(
      pipe(verify, tag<"Option", OptionDef<{ verified: boolean }>, "Some">("Some", "Option")),
    );
    assertExact<IsExact<ExtractInput<typeof action>, Option<{ artifact: string }>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Option<{ verified: boolean }>>>();
  });

  it("Option.andThen composes in pipe for chaining", () => {
    const action = pipe(
      tag<"Option", OptionDef<{ artifact: string }>, "Some">("Some", "Option"),
      O.andThen(pipe(verify, tag<"Option", OptionDef<{ verified: boolean }>, "Some">("Some", "Option"))),
    );
    assertExact<IsExact<ExtractInput<typeof action>, { artifact: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Option<{ verified: boolean }>>>();
  });

  it("Option.unwrapOr(defaultAction): Option<T> -> T", () => {
    const action = O.unwrapOr<string>(constant("fallback"));
    assertExact<IsExact<ExtractInput<typeof action>, Option<string>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, string>>();
  });

  it("Option.filter(predicate): Option<T> -> Option<T>", () => {
    const predicate = pipe(
      identity(),
      tag<"Option", OptionDef<string>, "Some">("Some", "Option"),
    );
    const action = O.filter<string>(predicate);
    assertExact<IsExact<ExtractInput<typeof action>, Option<string>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Option<string>>>();
  });

  it("Option.collect(): Option<T>[] -> T[]", () => {
    const action = O.collect<string>();
    assertExact<IsExact<ExtractInput<typeof action>, Option<string>[]>>();
    assertExact<IsExact<ExtractOutput<typeof action>, string[]>>();
  });

  it("Option.isSome(): Option<T> -> boolean", () => {
    const action = O.isSome<string>();
    assertExact<IsExact<ExtractInput<typeof action>, Option<string>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, boolean>>();
  });

  it("Option.isNone(): Option<T> -> boolean", () => {
    const action = O.isNone<number>();
    assertExact<IsExact<ExtractInput<typeof action>, Option<number>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, boolean>>();
  });

  it("full Option pipeline: construct -> map -> unwrapOr", () => {
    const action = pipe(
      tag<"Option", OptionDef<{ artifact: string }>, "Some">("Some", "Option"),
      O.map(verify),
      O.unwrapOr(constant({ verified: false })),
    );
    assertExact<IsExact<ExtractInput<typeof action>, { artifact: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { verified: boolean }>>();
  });

  it("forEach + Option.collect pipeline", () => {
    const action = pipe(
      forEach(O.map<{ artifact: string }, { verified: boolean }>(verify)),
      O.collect<{ verified: boolean }>(),
    );
    assertExact<IsExact<ExtractInput<typeof action>, Option<{ artifact: string }>[]>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { verified: boolean }[]>>();
  });

  it("Option.unwrap: Option<T> -> T", () => {
    const action = O.unwrap<number>();
    assertExact<IsExact<ExtractInput<typeof action>, Option<number>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, number>>();
  });

  it("Option.transpose: Option<Result<T,E>> -> Result<Option<T>,E>", () => {
    const action = O.transpose<string, number>();
    assertExact<IsExact<ExtractInput<typeof action>, Option<Result<string, number>>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Result<Option<string>, number>>>();
  });
});

// ---------------------------------------------------------------------------
// AST structure tests
// ---------------------------------------------------------------------------

describe("Option AST structure", () => {
  it("Option.map() produces Branch with Some and None cases", () => {
    const action = O.map(verify);
    expect(action.kind).toBe("Branch");
    const branchNode = action as { kind: "Branch"; cases: any };
    expect(Object.keys(branchNode.cases).toSorted()).toEqual(["None", "Some"]);
    const someCase = branchNode.cases["Some"];
    expect(someCase.kind).toBe("Chain");
    expect(someCase.first.handler.builtin.kind).toBe("GetField");
    expect(someCase.rest.kind).toBe("Chain");
    const noneCase = branchNode.cases["None"];
    expect(noneCase.kind).toBe("Chain");
    expect(noneCase.rest).toEqual(expectedTagAst("Option.None"));
  });

  it("Option.andThen() produces Branch with action Some and tag None", () => {
    const action = O.andThen(pipe(verify, tag<"Option", OptionDef<{ verified: boolean }>, "Some">("Some", "Option")));
    expect(action.kind).toBe("Branch");
    const branchNode = action as { kind: "Branch"; cases: any };
    expect(Object.keys(branchNode.cases).toSorted()).toEqual(["None", "Some"]);
    const someCase = branchNode.cases["Some"];
    expect(someCase.kind).toBe("Chain");
    expect(someCase.first.handler.builtin.kind).toBe("GetField");
    const noneCase = branchNode.cases["None"];
    expect(noneCase.rest).toEqual(expectedTagAst("Option.None"));
  });

  it("Option.unwrapOr() produces Branch with identity Some and default None", () => {
    const action = O.unwrapOr(constant("fallback"));
    expect(action.kind).toBe("Branch");
    const branchNode = action as { kind: "Branch"; cases: any };
    const someCase = branchNode.cases["Some"];
    expect(someCase.rest.handler.builtin.kind).toBe("Identity");
    const noneCase = branchNode.cases["None"];
    expect(noneCase.rest.handler.builtin.kind).toBe("Constant");
    expect(noneCase.rest.handler.builtin.value).toBe("fallback");
  });

  it("Option.filter() produces Branch with predicate Some and tag None", () => {
    const predicate = tag<"Option", OptionDef<string>, "Some">("Some", "Option");
    const action = O.filter(predicate);
    expect(action.kind).toBe("Branch");
    const branchNode = action as { kind: "Branch"; cases: any };
    expect(branchNode.cases["Some"].rest).toEqual(expectedTagAst("Option.Some"));
  });

  it("Option.collect() produces CollectSome builtin", () => {
    const action = O.collect<string>();
    expect(action.kind).toBe("Invoke");
    const invoke = action as { kind: "Invoke"; handler: any };
    expect(invoke.handler.builtin.kind).toBe("CollectSome");
  });

  it("Option.isSome() produces Branch with Constant(true) and Constant(false)", () => {
    const action = O.isSome<string>();
    expect(action.kind).toBe("Branch");
    const branchNode = action as { kind: "Branch"; cases: any };
    expect(branchNode.cases["Some"].rest.handler.builtin.value).toBe(true);
    expect(branchNode.cases["None"].rest.handler.builtin.value).toBe(false);
  });

  it("Option.isNone() is the inverse of isSome", () => {
    const action = O.isNone<string>();
    expect(action.kind).toBe("Branch");
    const branchNode = action as { kind: "Branch"; cases: any };
    expect(branchNode.cases["Some"].rest.handler.builtin.value).toBe(false);
    expect(branchNode.cases["None"].rest.handler.builtin.value).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Execution tests
// ---------------------------------------------------------------------------

describe("Option execution", () => {
  // -- Construction --
  it("Option.some wraps value", async () => {
    const result = await runPipeline(optionSome(42));
    expect(result).toEqual({ kind: "Option.Some", value: 42 });
  });

  it("Option.none produces None", async () => {
    const result = await runPipeline(optionNone<number>());
    expect(result).toEqual({ kind: "Option.None", value: null });
  });

  // -- map --
  it("Option.map on Some transforms value", async () => {
    const result = await runPipeline(
      pipe(optionSome(10), O.map(constant(20))),
    );
    expect(result).toEqual({ kind: "Option.Some", value: 20 });
  });

  it("Option.map on None stays None", async () => {
    const result = await runPipeline(
      pipe(optionNone<number>(), O.map(constant(999))),
    );
    expect(result).toEqual({ kind: "Option.None", value: null });
  });

  // -- andThen --
  it("Option.andThen on Some, action returns Some -> Some", async () => {
    const result = await runPipeline(
      pipe(
        optionSome(5),
        O.andThen<number, number>(pipe(constant(10), tag<"Option", OptionDef<number>, "Some">("Some", "Option"))),
      ),
    );
    expect(result).toEqual({ kind: "Option.Some", value: 10 });
  });

  it("Option.andThen on Some, action returns None -> None", async () => {
    const result = await runPipeline(
      pipe(
        optionSome(5),
        O.andThen<number, number>(pipe(drop, tag<"Option", OptionDef<number>, "None">("None", "Option"))),
      ),
    );
    expect(result).toEqual({ kind: "Option.None", value: null });
  });

  it("Option.andThen on None -> None", async () => {
    const result = await runPipeline(
      pipe(
        optionNone<number>(),
        O.andThen<number, number>(pipe(constant(10), tag<"Option", OptionDef<number>, "Some">("Some", "Option"))),
      ),
    );
    expect(result).toEqual({ kind: "Option.None", value: null });
  });

  // -- unwrap --
  it("Option.unwrap on Some extracts value", async () => {
    const result = await runPipeline(
      pipe(optionSome(42), O.unwrap()),
    );
    expect(result).toBe(42);
  });

  it("Option.unwrap on None panics", async () => {
    await expect(
      runPipeline(pipe(optionNone<number>(), O.unwrap())),
    ).rejects.toThrow();
  });

  // -- unwrapOr --
  it("Option.unwrapOr on Some returns value", async () => {
    const result = await runPipeline(
      pipe(optionSome(42), O.unwrapOr(constant(0))),
    );
    expect(result).toBe(42);
  });

  it("Option.unwrapOr on None runs fallback", async () => {
    const result = await runPipeline(
      pipe(optionNone<number>(), O.unwrapOr(constant(0))),
    );
    expect(result).toBe(0);
  });

  // -- filter --
  it("Option.filter on Some where predicate returns Some -> keeps", async () => {
    const result = await runPipeline(
      pipe(
        optionSome(42),
        O.filter<number>(pipe(identity(), tag<"Option", OptionDef<number>, "Some">("Some", "Option"))),
      ),
    );
    expect(result).toEqual({ kind: "Option.Some", value: 42 });
  });

  it("Option.filter on Some where predicate returns None -> drops", async () => {
    const result = await runPipeline(
      pipe(
        optionSome(42),
        O.filter<number>(pipe(drop, tag<"Option", OptionDef<number>, "None">("None", "Option"))),
      ),
    );
    expect(result).toEqual({ kind: "Option.None", value: null });
  });

  it("Option.filter on None -> None", async () => {
    const result = await runPipeline(
      pipe(
        optionNone<number>(),
        O.filter<number>(pipe(identity(), tag<"Option", OptionDef<number>, "Some">("Some", "Option"))),
      ),
    );
    expect(result).toEqual({ kind: "Option.None", value: null });
  });

  // -- collect --
  it("Option.collect on [Some(1), None, Some(3)] -> [1, 3]", async () => {
    const result = await runPipeline(
      pipe(
        constant([
          { kind: "Option.Some", value: 1 },
          { kind: "Option.None", value: null },
          { kind: "Option.Some", value: 3 },
        ] as Option<number>[]),
        O.collect(),
      ),
    );
    expect(result).toEqual([1, 3]);
  });

  it("Option.collect on [] -> []", async () => {
    const result = await runPipeline(
      pipe(constant([] as Option<number>[]), O.collect()),
    );
    expect(result).toEqual([]);
  });

  // -- isSome / isNone --
  it("Option.isSome on Some -> true", async () => {
    const result = await runPipeline(pipe(optionSome(1), O.isSome()));
    expect(result).toBe(true);
  });

  it("Option.isSome on None -> false", async () => {
    const result = await runPipeline(pipe(optionNone<number>(), O.isSome()));
    expect(result).toBe(false);
  });

  it("Option.isNone on Some -> false", async () => {
    const result = await runPipeline(pipe(optionSome(1), O.isNone()));
    expect(result).toBe(false);
  });

  it("Option.isNone on None -> true", async () => {
    const result = await runPipeline(pipe(optionNone<number>(), O.isNone()));
    expect(result).toBe(true);
  });

  // -- transpose --
  it("Option.transpose Some(Ok(x)) -> Ok(Some(x))", async () => {
    type Inner = Result<number, string>;
    const someOk = pipe(
      constant(42),
      tag<"Result", ResultDef<number, string>, "Ok">("Ok", "Result"),
      tag<"Option", OptionDef<Inner>, "Some">("Some", "Option"),
    );
    const result = await runPipeline(pipe(someOk, O.transpose()));
    expect(result).toEqual({
      kind: "Result.Ok",
      value: { kind: "Option.Some", value: 42 },
    });
  });

  it("Option.transpose Some(Err(e)) -> Err(e)", async () => {
    type Inner = Result<number, string>;
    const someErr = pipe(
      constant("oops"),
      tag<"Result", ResultDef<number, string>, "Err">("Err", "Result"),
      tag<"Option", OptionDef<Inner>, "Some">("Some", "Option"),
    );
    const result = await runPipeline(pipe(someErr, O.transpose()));
    expect(result).toEqual({ kind: "Result.Err", value: "oops" });
  });

  it("Option.transpose None -> Ok(None)", async () => {
    type Inner = Result<number, string>;
    const result = await runPipeline(
      pipe(optionNone<Inner>(), O.transpose()),
    );
    expect(result).toEqual({
      kind: "Result.Ok",
      value: { kind: "Option.None", value: null },
    });
  });

  // -- Postfix dispatch --
  it("postfix .map on Option output dispatches correctly", async () => {
    const result = await runPipeline(optionSome(42).map(constant(99)));
    expect(result).toEqual({ kind: "Option.Some", value: 99 });
  });

  it("postfix .unwrap on Option output", async () => {
    const result = await runPipeline(optionSome(42).unwrap());
    expect(result).toBe(42);
  });

  it("postfix .unwrapOr on Option output", async () => {
    const result = await runPipeline(optionNone<number>().unwrapOr(constant(99)));
    expect(result).toBe(99);
  });

  it("postfix .isSome on Option output", async () => {
    const result = await runPipeline(optionSome(42).isSome());
    expect(result).toBe(true);
  });

  it("postfix .isNone on Option output", async () => {
    const result = await runPipeline(optionSome(42).isNone());
    expect(result).toBe(false);
  });
});
