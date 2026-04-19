import { describe, it, expect, beforeEach } from "vitest";
import {
  type ExtractInput,
  type ExtractOutput,
  type Option,
  type Result,
  type ResultDef,
  type OptionDef,
  type TypedAction,
  pipe,
  resetEffectIdCounter,
  tryCatch,
  typedAction,
} from "../src/ast.js";
import { allocateRestartHandlerId } from "../src/effect-id.js";
import {
  constant,
  drop,
  identity,
  tag,
} from "../src/builtins/index.js";
import { Result as R } from "../src/result.js";
import { runPipeline } from "../src/run.js";
import { setup, deploy } from "./handlers.js";

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

// Helpers for constructing typed Result values in execution tests
function resultOk<TValue, TError = unknown>(value: TValue): TypedAction<any, Result<TValue, TError>> {
  return pipe(constant(value), tag<"Result", ResultDef<TValue, TError>, "Ok">("Ok", "Result"));
}
function resultErr<TValue, TError>(error: TError): TypedAction<any, Result<TValue, TError>> {
  return pipe(constant(error), tag<"Result", ResultDef<TValue, TError>, "Err">("Err", "Result"));
}

// ---------------------------------------------------------------------------
// Type tests
// ---------------------------------------------------------------------------

describe("Result constructor type info", () => {
  it("Result.ok() retains value type", () => {
    // @ts-expect-error — ok is currently a value, not a function
    const ok = R.ok<string>();
    assertExact<IsExact<ExtractInput<typeof ok>, string>>();
    assertExact<IsExact<ExtractOutput<typeof ok>, Result<string, unknown>>>();
  });

  it("Result.err() retains error type", () => {
    // @ts-expect-error — err is currently a value, not a function
    const err = R.err<unknown, number>();
    assertExact<IsExact<ExtractInput<typeof err>, number>>();
  });

  it("Result.ok() infers type from chain context", () => {
    // @ts-expect-error — ok is currently a value, not a function
    const result = constant("hello").then(R.ok());
    // @ts-expect-error — result is any until constructors are functions
    assertExact<IsExact<ExtractOutput<typeof result>, Result<string, unknown>>>();
  });
});

describe("Result types", () => {
  it("Result.map transforms Ok type, preserves Err type", () => {
    const action = R.map<string, number, boolean>(
      constant(42) as TypedAction<string, number>,
    );
    assertExact<IsExact<ExtractInput<typeof action>, Result<string, boolean>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Result<number, boolean>>>();
  });

  it("Result.mapErr transforms Err type, preserves Ok type", () => {
    const action = R.mapErr<string, number, boolean>(
      constant(true) as TypedAction<number, boolean>,
    );
    assertExact<IsExact<ExtractInput<typeof action>, Result<string, number>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Result<string, boolean>>>();
  });

  it("Result.andThen input is Result, output is Result with new Ok type", () => {
    const action = R.andThen<string, number, boolean>(
      constant({ kind: "Result.Ok" as const, value: 42 }) as TypedAction<string, Result<number, boolean>>,
    );
    assertExact<IsExact<ExtractInput<typeof action>, Result<string, boolean>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Result<number, boolean>>>();
  });

  it("Result.or input is Result, output has new Err type", () => {
    const action = R.or<string, number, boolean>(
      constant({ kind: "Result.Ok" as const, value: "x" }) as TypedAction<number, Result<string, boolean>>,
    );
    assertExact<IsExact<ExtractInput<typeof action>, Result<string, number>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Result<string, boolean>>>();
  });

  it("Result.and replaces Ok type, preserves Err type", () => {
    const action = R.and<string, number, boolean>(
      constant({ kind: "Result.Ok" as const, value: 42 }) as TypedAction<void, Result<number, boolean>>,
    );
    assertExact<IsExact<ExtractInput<typeof action>, Result<string, boolean>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Result<number, boolean>>>();
  });

  it("Result.unwrapOr extracts TValue from Result", () => {
    const action = R.unwrapOr<string, number>(
      constant("fallback") as TypedAction<number, string>,
    );
    assertExact<IsExact<ExtractInput<typeof action>, Result<string, number>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, string>>();
  });

  it("Result.toOption converts to Option<TValue>", () => {
    const action = R.toOption<string, number>();
    assertExact<IsExact<ExtractInput<typeof action>, Result<string, number>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Option<string>>>();
  });

  it("Result.toOptionErr converts to Option<TError>", () => {
    const action = R.toOptionErr<string, number>();
    assertExact<IsExact<ExtractInput<typeof action>, Result<string, number>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Option<number>>>();
  });

  it("Result.transpose swaps Result/Option nesting", () => {
    const action = R.transpose<string, number>();
    assertExact<IsExact<ExtractInput<typeof action>, Result<Option<string>, number>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Option<Result<string, number>>>>();
  });

  it("Result.isOk returns boolean", () => {
    const action = R.isOk<string, number>();
    assertExact<IsExact<ExtractInput<typeof action>, Result<string, number>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, boolean>>();
  });

  it("Result.isErr returns boolean", () => {
    const action = R.isErr<string, number>();
    assertExact<IsExact<ExtractInput<typeof action>, Result<string, number>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, boolean>>();
  });

  it("Result branches with Ok/Err cases", () => {
    const action = pipe(
      tag<"Result", ResultDef<string, number>, "Ok">("Ok", "Result"),
      R.map<string, number, number>(constant(42) as TypedAction<string, number>),
      R.unwrapOr<number, number>(identity()),
    );
    assertExact<IsExact<ExtractInput<typeof action>, string>>();
    assertExact<IsExact<ExtractOutput<typeof action>, number>>();
  });
});

// ---------------------------------------------------------------------------
// Result.unwrapOr with throw tokens
// ---------------------------------------------------------------------------

describe("Result.unwrapOr with throw tokens", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("Result.unwrapOr accepts throw token with explicit types", () => {
    const throwToken = typedAction<string, never>({ kind: "RestartPerform", restart_handler_id: allocateRestartHandlerId() });
    const action = R.unwrapOr<string, string>(throwToken);
    assertExact<IsExact<ExtractInput<typeof action>, Result<string, string>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, string>>();
  });

  it(".unwrapOr() infers types from this constraint", () => {
    const resultAction = identity() as TypedAction<string, Result<string, number>>;
    const throwToken = typedAction<number, never>({ kind: "RestartPerform", restart_handler_id: allocateRestartHandlerId() });
    const action = resultAction.unwrapOr(throwToken);
    assertExact<IsExact<ExtractInput<typeof action>, string>>();
    assertExact<IsExact<ExtractOutput<typeof action>, string>>();
  });

  it(".unwrapOr() composes in tryCatch pipeline", () => {
    const handler = identity() as TypedAction<
      { data: string },
      Result<{ data: string }, { code: number }>
    >;
    const action = tryCatch(
      (throwError) => handler.unwrapOr(throwError),
      pipe(drop, constant({ data: "fallback" })),
    );
    assertExact<IsExact<ExtractInput<typeof action>, { data: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { data: string }>>();
  });

  it(".unwrapOr() chains into further pipeline steps", () => {
    const handler = identity() as TypedAction<
      { artifact: string },
      Result<{ verified: boolean }, string>
    >;
    const action = tryCatch(
      (throwError) => pipe(
        handler.unwrapOr(throwError),
        deploy,
      ),
      pipe(drop, constant({ deployed: false })),
    );
    assertExact<IsExact<ExtractInput<typeof action>, { artifact: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { deployed: boolean }>>();
  });

  it(".unwrapOr() produces Chain AST node", () => {
    const resultAction = identity() as TypedAction<void, Result<string, string>>;
    const throwToken = typedAction<string, never>({ kind: "RestartPerform", restart_handler_id: allocateRestartHandlerId() });
    const action = resultAction.unwrapOr(throwToken);
    expect(action.kind).toBe("Chain");
  });

  it("rejects .unwrapOr() on non-Result output", () => {
    // @ts-expect-error — unwrapOr requires Option or Result output
    setup.unwrapOr(drop);
  });
});

// ---------------------------------------------------------------------------
// AST structure tests
// ---------------------------------------------------------------------------

describe("Result AST structure", () => {
  it("Result.map(action) desugars correctly", () => {
    const action = R.map(setup);
    expect(action).toEqual({
      kind: "Branch",
      cases: {
        Ok: {
          kind: "Chain",
          first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "GetField", field: "value" } } },
          rest: { kind: "Chain", first: setup, rest: expectedTagAst("Result.Ok") },
        },
        Err: {
          kind: "Chain",
          first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "GetField", field: "value" } } },
          rest: expectedTagAst("Result.Err"),
        },
      },
    });
  });

  it("Result.mapErr(action) desugars correctly", () => {
    const action = R.mapErr(setup);
    expect(action).toEqual({
      kind: "Branch",
      cases: {
        Ok: {
          kind: "Chain",
          first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "GetField", field: "value" } } },
          rest: expectedTagAst("Result.Ok"),
        },
        Err: {
          kind: "Chain",
          first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "GetField", field: "value" } } },
          rest: { kind: "Chain", first: setup, rest: expectedTagAst("Result.Err") },
        },
      },
    });
  });

  it("Result.andThen(action) desugars correctly", () => {
    const inner = tag<"Result", ResultDef<string, string>, "Ok">("Ok", "Result");
    const result = R.andThen(inner);
    const branchNode = result as any;
    expect(branchNode.kind).toBe("Branch");
    expect(branchNode.cases.Ok.rest).toEqual(inner);
    expect(branchNode.cases.Err.rest).toEqual(expectedTagAst("Result.Err"));
  });

  it("Result.or(fallback) desugars correctly", () => {
    const fallback = tag<"Result", ResultDef<string, string>, "Ok">("Ok", "Result");
    const result = R.or(fallback);
    const branchNode = result as any;
    expect(branchNode.kind).toBe("Branch");
    expect(branchNode.cases.Ok.rest).toEqual(expectedTagAst("Result.Ok"));
    expect(branchNode.cases.Err.rest).toBe(fallback);
  });

  it("Result.and(other) desugars correctly", () => {
    const other = pipe(constant("replacement"), tag<"Result", ResultDef<string, string>, "Ok">("Ok", "Result"));
    const result = R.and(other);
    const branchNode = result as any;
    expect(branchNode.kind).toBe("Branch");
    const okBody = branchNode.cases.Ok.rest;
    expect(okBody.kind).toBe("Chain");
    expect(okBody.first.handler.builtin.kind).toBe("Drop");
    expect(okBody.rest).toBe(other);
  });

  it("Result.unwrapOr(default) desugars correctly", () => {
    const fallback = constant("default");
    const action = R.unwrapOr(fallback);
    const branchNode = action as any;
    expect(branchNode.kind).toBe("Branch");
    expect(branchNode.cases.Ok.rest.handler.builtin.kind).toBe("Identity");
    expect(branchNode.cases.Err.rest).toBe(fallback);
  });

  it("Result.toOption() desugars correctly", () => {
    const action = R.toOption();
    const branchNode = action as any;
    expect(branchNode.kind).toBe("Branch");
    expect(branchNode.cases.Ok.rest).toEqual(expectedTagAst("Option.Some"));
    expect(branchNode.cases.Err.rest.first.handler.builtin.kind).toBe("Drop");
    expect(branchNode.cases.Err.rest.rest).toEqual(expectedTagAst("Option.None"));
  });

  it("Result.toOptionErr() desugars correctly", () => {
    const action = R.toOptionErr();
    const branchNode = action as any;
    expect(branchNode.kind).toBe("Branch");
    expect(branchNode.cases.Ok.rest.first.handler.builtin.kind).toBe("Drop");
    expect(branchNode.cases.Ok.rest.rest).toEqual(expectedTagAst("Option.None"));
    expect(branchNode.cases.Err.rest).toEqual(expectedTagAst("Option.Some"));
  });

  it("Result.isOk() desugars correctly", () => {
    const action = R.isOk();
    const branchNode = action as any;
    expect(branchNode.kind).toBe("Branch");
    expect(branchNode.cases.Ok.rest.handler.builtin.value).toBe(true);
    expect(branchNode.cases.Err.rest.handler.builtin.value).toBe(false);
  });

  it("Result.isErr() desugars correctly", () => {
    const action = R.isErr();
    const branchNode = action as any;
    expect(branchNode.kind).toBe("Branch");
    expect(branchNode.cases.Ok.rest.handler.builtin.value).toBe(false);
    expect(branchNode.cases.Err.rest.handler.builtin.value).toBe(true);
  });

  it("Result.transpose() desugars to nested branches", () => {
    const action = R.transpose();
    const branchNode = action as any;
    expect(branchNode.kind).toBe("Branch");
    const okBody = branchNode.cases.Ok.rest;
    expect(okBody.kind).toBe("Branch");
    expect(okBody.cases.Some).toBeDefined();
    expect(okBody.cases.None).toBeDefined();
    const errBody = branchNode.cases.Err.rest;
    expect(errBody.kind).toBe("Chain");
    expect(errBody.first).toEqual(expectedTagAst("Result.Err"));
    expect(errBody.rest).toEqual(expectedTagAst("Option.Some"));
  });
});

// ---------------------------------------------------------------------------
// Execution tests
// ---------------------------------------------------------------------------

describe("Result execution", () => {
  // -- Construction --
  it("Result.ok wraps value", async () => {
    const result = await runPipeline(resultOk(42));
    expect(result).toEqual({ kind: "Result.Ok", value: 42 });
  });

  it("Result.err wraps error", async () => {
    const result = await runPipeline(resultErr("oops"));
    expect(result).toEqual({ kind: "Result.Err", value: "oops" });
  });

  // -- map --
  it("Result.map on Ok transforms value", async () => {
    const result = await runPipeline(
      pipe(resultOk(10), R.map(constant(20))),
    );
    expect(result).toEqual({ kind: "Result.Ok", value: 20 });
  });

  it("Result.map on Err stays Err", async () => {
    const result = await runPipeline(
      pipe(resultErr<number, string>("fail"), R.map(constant(999))),
    );
    expect(result).toEqual({ kind: "Result.Err", value: "fail" });
  });

  // -- mapErr --
  it("Result.mapErr on Ok stays Ok", async () => {
    const result = await runPipeline(
      pipe(resultOk(42), R.mapErr(constant("transformed"))),
    );
    expect(result).toEqual({ kind: "Result.Ok", value: 42 });
  });

  it("Result.mapErr on Err transforms error", async () => {
    const result = await runPipeline(
      pipe(resultErr<number, string>("fail"), R.mapErr(constant("transformed"))),
    );
    expect(result).toEqual({ kind: "Result.Err", value: "transformed" });
  });

  // -- andThen --
  it("Result.andThen on Ok chains to inner Result", async () => {
    const result = await runPipeline(
      pipe(
        resultOk<number, string>(5),
        R.andThen<number, number, string>(
          pipe(constant(10), tag<"Result", ResultDef<number, string>, "Ok">("Ok", "Result")),
        ),
      ),
    );
    expect(result).toEqual({ kind: "Result.Ok", value: 10 });
  });

  it("Result.andThen on Err propagates", async () => {
    const result = await runPipeline(
      pipe(
        resultErr<number, string>("fail"),
        R.andThen<number, number, string>(
          pipe(constant(10), tag<"Result", ResultDef<number, string>, "Ok">("Ok", "Result")),
        ),
      ),
    );
    expect(result).toEqual({ kind: "Result.Err", value: "fail" });
  });

  // -- or --
  it("Result.or on Ok stays Ok", async () => {
    const result = await runPipeline(
      pipe(
        resultOk<number, string>(42),
        R.or<number, string, string>(
          pipe(constant(99), tag<"Result", ResultDef<number, string>, "Ok">("Ok", "Result")),
        ),
      ),
    );
    expect(result).toEqual({ kind: "Result.Ok", value: 42 });
  });

  it("Result.or on Err applies fallback", async () => {
    const result = await runPipeline(
      pipe(
        resultErr<number, string>("fail"),
        R.or<number, string, string>(
          pipe(constant(99), tag<"Result", ResultDef<number, string>, "Ok">("Ok", "Result")),
        ),
      ),
    );
    expect(result).toEqual({ kind: "Result.Ok", value: 99 });
  });

  // -- and --
  it("Result.and on Ok replaces with other", async () => {
    const result = await runPipeline(
      pipe(
        resultOk<number, string>(42),
        R.and<number, string, string>(
          pipe(constant("replaced"), tag<"Result", ResultDef<string, string>, "Ok">("Ok", "Result")),
        ),
      ),
    );
    expect(result).toEqual({ kind: "Result.Ok", value: "replaced" });
  });

  it("Result.and on Err stays Err", async () => {
    const result = await runPipeline(
      pipe(
        resultErr<number, string>("fail"),
        R.and<number, string, string>(
          pipe(constant("replaced"), tag<"Result", ResultDef<string, string>, "Ok">("Ok", "Result")),
        ),
      ),
    );
    expect(result).toEqual({ kind: "Result.Err", value: "fail" });
  });

  // -- unwrap --
  it("Result.unwrap on Ok extracts value", async () => {
    const result = await runPipeline(
      pipe(resultOk(42), R.unwrap()),
    );
    expect(result).toBe(42);
  });

  it("Result.unwrap on Err panics", async () => {
    await expect(
      runPipeline(pipe(resultErr("fail"), R.unwrap())),
    ).rejects.toThrow();
  });

  // -- unwrapOr --
  it("Result.unwrapOr on Ok returns value", async () => {
    const result = await runPipeline(
      pipe(resultOk(42), R.unwrapOr(constant(0))),
    );
    expect(result).toBe(42);
  });

  it("Result.unwrapOr on Err runs fallback", async () => {
    const result = await runPipeline(
      pipe(resultErr<number, string>("fail"), R.unwrapOr(constant(0))),
    );
    expect(result).toBe(0);
  });

  // -- toOption / toOptionErr --
  it("Result.toOption on Ok -> Some", async () => {
    const result = await runPipeline(
      pipe(resultOk(42), R.toOption()),
    );
    expect(result).toEqual({ kind: "Option.Some", value: 42 });
  });

  it("Result.toOption on Err -> None", async () => {
    const result = await runPipeline(
      pipe(resultErr("fail"), R.toOption()),
    );
    expect(result).toEqual({ kind: "Option.None", value: null });
  });

  it("Result.toOptionErr on Ok -> None", async () => {
    const result = await runPipeline(
      pipe(resultOk(42), R.toOptionErr()),
    );
    expect(result).toEqual({ kind: "Option.None", value: null });
  });

  it("Result.toOptionErr on Err -> Some", async () => {
    const result = await runPipeline(
      pipe(resultErr("fail"), R.toOptionErr()),
    );
    expect(result).toEqual({ kind: "Option.Some", value: "fail" });
  });

  // -- transpose --
  it("Result.transpose Ok(Some(x)) -> Some(Ok(x))", async () => {
    type Inner = Option<number>;
    const okSome = pipe(
      constant(42),
      tag<"Option", OptionDef<number>, "Some">("Some", "Option"),
      tag<"Result", ResultDef<Inner, string>, "Ok">("Ok", "Result"),
    );
    const result = await runPipeline(pipe(okSome, R.transpose()));
    expect(result).toEqual({
      kind: "Option.Some",
      value: { kind: "Result.Ok", value: 42 },
    });
  });

  it("Result.transpose Ok(None) -> None", async () => {
    type Inner = Option<number>;
    const okNone = pipe(
      constant(null),
      tag<"Option", OptionDef<number>, "None">("None", "Option"),
      tag<"Result", ResultDef<Inner, string>, "Ok">("Ok", "Result"),
    );
    const result = await runPipeline(pipe(okNone, R.transpose()));
    expect(result).toEqual({ kind: "Option.None", value: null });
  });

  it("Result.transpose Err(e) -> Some(Err(e))", async () => {
    type Inner = Option<number>;
    const errVal = pipe(
      constant("oops"),
      tag<"Result", ResultDef<Inner, string>, "Err">("Err", "Result"),
    );
    const result = await runPipeline(pipe(errVal, R.transpose()));
    expect(result).toEqual({
      kind: "Option.Some",
      value: { kind: "Result.Err", value: "oops" },
    });
  });

  // -- isOk / isErr --
  it("Result.isOk on Ok -> true", async () => {
    const result = await runPipeline(pipe(resultOk(1), R.isOk()));
    expect(result).toBe(true);
  });

  it("Result.isOk on Err -> false", async () => {
    const result = await runPipeline(pipe(resultErr("e"), R.isOk()));
    expect(result).toBe(false);
  });

  it("Result.isErr on Ok -> false", async () => {
    const result = await runPipeline(pipe(resultOk(1), R.isErr()));
    expect(result).toBe(false);
  });

  it("Result.isErr on Err -> true", async () => {
    const result = await runPipeline(pipe(resultErr("e"), R.isErr()));
    expect(result).toBe(true);
  });

  // -- Postfix dispatch --
  it("postfix .map on Result output dispatches correctly", async () => {
    const result = await runPipeline(resultOk(42).map(constant(99)));
    expect(result).toEqual({ kind: "Result.Ok", value: 99 });
  });

  it("postfix .unwrap on Result output", async () => {
    const result = await runPipeline(resultOk(42).unwrap());
    expect(result).toBe(42);
  });

  it("postfix .unwrapOr on Result output", async () => {
    const result = await runPipeline(resultErr<number, string>("fail").unwrapOr(constant(99)));
    expect(result).toBe(99);
  });

  it("postfix .isOk on Result output", async () => {
    const result = await runPipeline(resultOk(42).isOk());
    expect(result).toBe(true);
  });

  it("postfix .isErr on Result output", async () => {
    const result = await runPipeline(resultOk(42).isErr());
    expect(result).toBe(false);
  });
});
