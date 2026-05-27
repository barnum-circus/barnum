import { describe, it, expect } from "vitest";
import {
  type ExtractInput,
  type ExtractOutput,
  pipe,
  type TypedAction,
} from "../src/ast.js";
import {
  allObject,
  constant,
  getField,
  wrapInField,
  pick,
} from "../src/builtins/index.js";
import { runPipeline } from "../src/run.js";
import { setup } from "./handlers.js";
import { assertExact, type IsExact } from "./type-utils.js";

// ---------------------------------------------------------------------------
// Type tests
// ---------------------------------------------------------------------------

describe("struct type tests", () => {
  it("getField: { key: V } -> V", () => {
    const action = getField<{ name: string; age: number }, "name">("name");
    assertExact<
      IsExact<ExtractInput<typeof action>, { name: string; age: number }>
    >();
    assertExact<IsExact<ExtractOutput<typeof action>, string>>();
    expect(action.kind).toBe("Invoke");
  });

  it("wrapInField: T -> Record<F, T>", () => {
    const action = wrapInField<"foo", number>("foo");
    assertExact<IsExact<ExtractInput<typeof action>, number>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Record<"foo", number>>>();
    expect(action.kind).toBe("Invoke");
  });

  it("pick: Obj -> Pick<Obj, Keys>", () => {
    const action = pick<{ a: number; b: string; c: boolean }, ["a", "b"]>(
      "a",
      "b",
    );
    assertExact<
      IsExact<ExtractInput<typeof action>, { a: number; b: string; c: boolean }>
    >();
    assertExact<
      IsExact<
        ExtractOutput<typeof action>,
        Pick<{ a: number; b: string; c: boolean }, "a" | "b">
      >
    >();
  });

  it("allObject: TInput -> { [K]: ExtractOutput<TActions[K]> }", () => {
    const action = allObject({
      name: constant("hello"),
      count: constant(42),
    });
    assertExact<
      IsExact<ExtractOutput<typeof action>, { name: string; count: number }>
    >();
  });

  it("allObjects has the correct input type (one key)", () => {
    const acceptsNumber: TypedAction<number, null> = null as any;

    const action = allObject({
      name: acceptsNumber,
    });

    assertExact<IsExact<ExtractInput<typeof action>, number>>();
  });

  it("allObjects has the correct input type (multiple keys, identical input)", () => {
    const acceptsNumber: TypedAction<number, null> = null as any;

    const action = allObject({
      foo: acceptsNumber,
      bar: acceptsNumber,
    });

    assertExact<IsExact<ExtractInput<typeof action>, number>>();
  });

  it("allObjects rejects invalid inputs (multiple keys, overlapping input)", () => {
    const acceptsNumber: TypedAction<number, null> = null as any;
    const acceptsStringOrNumber: TypedAction<string | number, null> =
      null as any;

    // You may want or expect this to infer that allObject accepts numbers! But that would open
    // the door to Pipeable<{ foo }> accepting { foo, bar }, which we do not want.

    allObject({
      // @ts-expect-error number != number | string
      foo: acceptsNumber,
      bar: acceptsStringOrNumber,
    });
  });

  it("allObjects rejects invalid inputs (multiple keys, non-overlapping input)", () => {
    const acceptsNumber: TypedAction<number, null> = null as any;
    const acceptsString: TypedAction<string, null> = null as any;

    allObject({
      // @ts-expect-error number != string
      foo: acceptsNumber,
      // @ts-expect-error number != string
      bar: acceptsString,
    });
  });
});

// ---------------------------------------------------------------------------
// AST structure tests
// ---------------------------------------------------------------------------

describe("struct AST structure", () => {
  it(".getField() produces Chain -> GetField AST", () => {
    const action = setup.getField("project");
    expect(action.kind).toBe("Chain");
    const chain = action as { kind: "Chain"; first: any; rest: any };
    expect(chain.first.kind).toBe("Invoke");
    expect(chain.rest.kind).toBe("Invoke");
    expect(chain.rest.handler.builtin.kind).toBe("GetField");
    expect(chain.rest.handler.builtin.field).toBe("project");
  });
});

// ---------------------------------------------------------------------------
// Execution tests
// ---------------------------------------------------------------------------

describe("struct execution", () => {
  it("getField extracts a field from an object", async () => {
    const result = await runPipeline(
      pipe(constant({ name: "alice", age: 30 }), getField("name")),
    );
    expect(result).toBe("alice");
  });

  it("wrapInField wraps a value in a named field", async () => {
    const result = await runPipeline(pipe(constant(42), wrapInField("foo")));
    expect(result).toEqual({ foo: 42 });
  });

  it("wrapInField with complex object value", async () => {
    const result = await runPipeline(
      pipe(constant({ x: [1, 2] }), wrapInField("data")),
    );
    expect(result).toEqual({ data: { x: [1, 2] } });
  });

  it("pick selects named fields", async () => {
    const result = await runPipeline(
      pipe(constant({ a: 1, b: 2, c: 3 }), pick("a", "b")),
    );
    expect(result).toEqual({ a: 1, b: 2 });
  });

  it("allObject runs actions concurrently and collects into an object", async () => {
    const result = await runPipeline(
      pipe(
        constant({ x: 10 }),
        allObject({
          val: getField("x"),
          wrapped: pipe(getField("x"), wrapInField("inner")),
          fixed: constant("hello"),
        }),
      ),
    );
    expect(result).toEqual({ val: 10, wrapped: { inner: 10 }, fixed: "hello" });
  });

  it("allObject with single action", async () => {
    const result = await runPipeline(
      pipe(constant(42), allObject({ answer: constant("yes") })),
    );
    expect(result).toEqual({ answer: "yes" });
  });
});
