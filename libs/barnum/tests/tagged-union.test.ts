import { describe, expect, it } from "vitest";
import { z } from "zod";
import {
  type ExtractOutput,
  type TaggedUnion,
  branch,
  branchFamily,
  pipe,
} from "../src/ast.js";
import {
  constant,
  extractPrefix,
  identity,
  tag,
  taggedUnionSchema,
} from "../src/builtins/index.js";
import { verify } from "./handlers.js";
import { type IsExact, assertExact } from "./type-utils.js";

/**
 * Build the expected AST for `tag(kind)`.
 */
function expectedTagAst(kind: string) {
  return {
    kind: "Chain",
    first: {
      kind: "All",
      actions: [
        {
          kind: "Chain",
          first: {
            kind: "Invoke",
            handler: {
              kind: "Builtin",
              builtin: { kind: "Constant", value: kind },
            },
          },
          rest: {
            kind: "Invoke",
            handler: {
              kind: "Builtin",
              builtin: { kind: "WrapInField", field: "kind" },
            },
          },
        },
        {
          kind: "Invoke",
          handler: {
            kind: "Builtin",
            builtin: { kind: "WrapInField", field: "value" },
          },
        },
      ],
    },
    rest: {
      kind: "Invoke",
      handler: { kind: "Builtin", builtin: { kind: "Merge" } },
    },
  };
}

// ---------------------------------------------------------------------------
// Type tests
// ---------------------------------------------------------------------------

describe("tagged-union type tests", () => {
  it("tag: T -> TaggedUnion<TEnumName, TDef>", () => {
    type Def = { Ok: string; Err: number };
    const action = tag<"Result", Def, "Ok">("Ok", "Result");
    assertExact<
      IsExact<ExtractOutput<typeof action>, TaggedUnion<"Result", Def>>
    >();
  });

  it("extractPrefix: produces untyped TypedAction", () => {
    const action = extractPrefix();
    expect(action.kind).toBe("Invoke");
  });

  it("taggedUnionSchema: produces correct Zod type", () => {
    const schema = taggedUnionSchema("Status", {
      Active: z.string(),
      Inactive: z.null(),
    });
    type SchemaType = z.infer<typeof schema>;
    assertExact<
      IsExact<
        SchemaType,
        TaggedUnion<"Status", { Active: string; Inactive: void }>
      >
    >();
  });
});

// ---------------------------------------------------------------------------
// AST structure tests
// ---------------------------------------------------------------------------

describe("tagged-union AST structure", () => {
  it(".tag() produces Chain -> tag composition AST", () => {
    const action = verify.tag<
      "VerifyResult",
      { Ok: { verified: boolean } },
      "Ok"
    >("Ok", "VerifyResult");
    expect(action.kind).toBe("Chain");
    const chain = action as { kind: "Chain"; first: any; rest: any };
    expect(chain.first.kind).toBe("Invoke");
    expect(chain.rest).toEqual(expectedTagAst("VerifyResult.Ok"));
  });

  it("postfix methods are chainable (tag-related)", () => {
    const action = verify.tag<
      "VerifyResult",
      { Ok: { verified: boolean } },
      "Ok"
    >("Ok", "VerifyResult");
    expect(action.kind).toBe("Chain");
  });
});

// ---------------------------------------------------------------------------
// Execution tests
// ---------------------------------------------------------------------------

describe("tagged-union execution", () => {
  it("tag('Ok', 'Result')(42) -> {kind: 'Result.Ok', value: 42}", async () => {
    const result = await pipe(constant(42), tag("Ok", "Result")).run();
    expect(result).toEqual({ kind: "Result.Ok", value: 42 });
  });

  it("tag('None', 'Option')(null) -> {kind: 'Option.None', value: null}", async () => {
    const result = await pipe(constant(null), tag("None", "Option")).run();
    expect(result).toEqual({ kind: "Option.None", value: null });
  });

  it("tag('Foo', 'MyEnum')('bar') -> {kind: 'MyEnum.Foo', value: 'bar'}", async () => {
    const result = await pipe(constant("bar"), tag("Foo", "MyEnum")).run();
    expect(result).toEqual({ kind: "MyEnum.Foo", value: "bar" });
  });

  it("extractPrefix on Result.Ok -> {kind: 'Result', value: ...}", async () => {
    const result = await pipe(
      constant({ kind: "Result.Ok", value: 42 }),
      extractPrefix(),
    ).run();
    expect(result).toEqual({
      kind: "Result",
      value: { kind: "Result.Ok", value: 42 },
    });
  });

  it("extractPrefix on kind with no dot -> prefix is entire kind", async () => {
    const result = await pipe(
      constant({ kind: "NoDot", value: 1 }),
      extractPrefix(),
    ).run();
    expect(result).toEqual({
      kind: "NoDot",
      value: { kind: "NoDot", value: 1 },
    });
  });

  it("branchFamily dispatches to 'Result' arm for Result.Ok input", async () => {
    const result = await pipe(
      constant({ kind: "Result.Ok", value: 42 }),
      branchFamily({
        Result: branch({ Ok: identity(), Err: identity() }),
        Option: branch({ Some: identity(), None: identity() }),
      }),
    ).run();
    expect(result).toBe(42);
  });

  it("branchFamily dispatches to 'Option' arm for Option.Some input", async () => {
    const result = await pipe(
      constant({ kind: "Option.Some", value: "hello" }),
      branchFamily({
        Result: branch({ Ok: identity(), Err: identity() }),
        Option: branch({ Some: identity(), None: identity() }),
      }),
    ).run();
    expect(result).toBe("hello");
  });

  it("branchFamily dispatches to 'Array' arm for bare array input", async () => {
    const result = await pipe(
      constant([10, 20, 30]),
      branchFamily({
        Array: identity(),
        Option: branch({ Some: identity(), None: identity() }),
      }),
    ).run();
    expect(result).toEqual([10, 20, 30]);
  });

  it("branchFamily dispatches to 'Array' arm for empty array input", async () => {
    const result = await pipe(
      constant([]),
      branchFamily({
        Array: identity(),
        Option: branch({ Some: identity(), None: identity() }),
      }),
    ).run();
    expect(result).toEqual([]);
  });

  it("taggedUnionSchema validates correct values", () => {
    const schema = taggedUnionSchema("Result", {
      Ok: z.number(),
      Err: z.string(),
    });
    expect(schema.parse({ kind: "Result.Ok", value: 42 })).toEqual({
      kind: "Result.Ok",
      value: 42,
    });
    expect(schema.parse({ kind: "Result.Err", value: "oops" })).toEqual({
      kind: "Result.Err",
      value: "oops",
    });
  });

  it("taggedUnionSchema rejects incorrect values", () => {
    const schema = taggedUnionSchema("Result", {
      Ok: z.number(),
      Err: z.string(),
    });
    expect(() => schema.parse({ kind: "Result.Ok", value: "wrong" })).toThrow();
    expect(() => schema.parse({ kind: "Result.Nope", value: 1 })).toThrow();
  });

  it("taggedUnionSchema with void variant (z.null())", () => {
    const schema = taggedUnionSchema("Status", {
      Active: z.string(),
      Inactive: z.null(),
    });
    expect(schema.parse({ kind: "Status.Active", value: "running" })).toEqual({
      kind: "Status.Active",
      value: "running",
    });
    expect(schema.parse({ kind: "Status.Inactive", value: null })).toEqual({
      kind: "Status.Inactive",
      value: null,
    });
  });
});
