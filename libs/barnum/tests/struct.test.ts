import { describe, expect, it } from "vitest";
import { type TypedAction, pipe } from "../src/ast.js";
import {
  allObject,
  constant,
  getField,
  pick,
  wrapInField,
} from "../src/builtins/index.js";
import { setup } from "./handlers.js";
import { assertIO } from "./type-utils.js";

// ---------------------------------------------------------------------------
// Type tests
// ---------------------------------------------------------------------------

describe("struct type tests", () => {
  it("getField: { key: V } -> V", () => {
    const action = getField<{ name: string; age: number }, "name">("name");
    assertIO<typeof action, { name: string; age: number }, string>();
    expect(action.kind).toBe("Invoke");
  });

  it("wrapInField: T -> Record<F, T>", () => {
    const action = wrapInField<"foo", number>("foo");
    assertIO<typeof action, number, Record<"foo", number>>();
    expect(action.kind).toBe("Invoke");
  });

  it("pick: Obj -> Pick<Obj, Keys>", () => {
    const action = pick<{ a: number; b: string; c: boolean }, ["a", "b"]>(
      "a",
      "b",
    );
    assertIO<
      typeof action,
      { a: number; b: string; c: boolean },
      Pick<{ a: number; b: string; c: boolean }, "a" | "b">
    >();
  });

  it("allObject: TInput -> { [K]: ExtractOutput<TActions[K]> }", () => {
    const action = allObject({
      name: constant("hello"),
      count: constant(42),
    });
    assertIO<typeof action, any, { name: string; count: number }>();
  });

  it("allObjects has the correct input type (one key)", () => {
    const acceptsNumber: TypedAction<number, null> = null as any;

    const action = allObject({
      name: acceptsNumber,
    });

    assertIO<typeof action, number, { name: null }>();
  });

  it("allObjects has the correct input type (multiple keys, identical input)", () => {
    const acceptsNumber: TypedAction<number, null> = null as any;

    const action = allObject({
      foo: acceptsNumber,
      bar: acceptsNumber,
    });

    assertIO<typeof action, number, { foo: null; bar: null }>();
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
    const result = await pipe(
      constant({ name: "alice", age: 30 }),
      getField("name"),
    ).run();
    expect(result).toBe("alice");
  });

  it("wrapInField wraps a value in a named field", async () => {
    const result = await pipe(constant(42), wrapInField("foo")).run();
    expect(result).toEqual({ foo: 42 });
  });

  it("wrapInField with complex object value", async () => {
    const result = await pipe(
      constant({ x: [1, 2] }),
      wrapInField("data"),
    ).run();
    expect(result).toEqual({ data: { x: [1, 2] } });
  });

  it("pick selects named fields", async () => {
    const result = await pipe(
      constant({ a: 1, b: 2, c: 3 }),
      pick("a", "b"),
    ).run();
    expect(result).toEqual({ a: 1, b: 2 });
  });

  it("allObject runs actions concurrently and collects into an object", async () => {
    const result = await pipe(
      constant({ x: 10 }),
      allObject({
        val: getField("x"),
        wrapped: pipe(getField("x"), wrapInField("inner")),
        fixed: constant("hello"),
      }),
    ).run();
    expect(result).toEqual({ val: 10, wrapped: { inner: 10 }, fixed: "hello" });
  });

  it("allObject with single action", async () => {
    const result = await pipe(
      constant(42),
      allObject({ answer: constant("yes") }),
    ).run();
    expect(result).toEqual({ answer: "yes" });
  });
});
