import { describe, it, expect } from "vitest";
import { z } from "zod";
import {
  type TaggedUnion,
  type ExtractOutput,
  pipe,
  branch,
  branchFamily,
} from "../src/ast.js";
import {
  constant,
  identity,
  tag,
  extractPrefix,
  taggedUnionSchema,
} from "../src/builtins/index.js";
import { runPipeline } from "../src/run.js";
import { verify } from "./handlers.js";

// ---------------------------------------------------------------------------
// Type assertion helpers (compile-time only)
// ---------------------------------------------------------------------------

type IsExact<T, U> = [T] extends [U] ? ([U] extends [T] ? true : false) : false;

// eslint-disable-next-line @typescript-eslint/no-unused-vars
function assertExact<_T extends true>(): void {}

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
          first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Constant", value: kind } } },
          rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "WrapInField", field: "kind" } } },
        },
        { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "WrapInField", field: "value" } } },
      ],
    },
    rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Merge" } } },
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
    const action = verify.tag<"VerifyResult", { Ok: { verified: boolean } }, "Ok">("Ok", "VerifyResult");
    expect(action.kind).toBe("Chain");
    const chain = action as { kind: "Chain"; first: any; rest: any };
    expect(chain.first.kind).toBe("Invoke");
    expect(chain.rest).toEqual(expectedTagAst("VerifyResult.Ok"));
  });

  it("postfix methods are chainable (tag-related)", () => {
    const action = verify.tag<"VerifyResult", { Ok: { verified: boolean } }, "Ok">("Ok", "VerifyResult");
    expect(action.kind).toBe("Chain");
  });
});

// ---------------------------------------------------------------------------
// Execution tests
// ---------------------------------------------------------------------------

describe("tagged-union execution", () => {
  it("tag('Ok', 'Result')(42) -> {kind: 'Result.Ok', value: 42}", async () => {
    const result = await runPipeline(
      pipe(constant(42), tag("Ok", "Result")),
    );
    expect(result).toEqual({ kind: "Result.Ok", value: 42 });
  });

  it("tag('None', 'Option')(null) -> {kind: 'Option.None', value: null}", async () => {
    const result = await runPipeline(
      pipe(constant(null), tag("None", "Option")),
    );
    expect(result).toEqual({ kind: "Option.None", value: null });
  });

  it("tag('Foo', 'MyEnum')('bar') -> {kind: 'MyEnum.Foo', value: 'bar'}", async () => {
    const result = await runPipeline(
      pipe(constant("bar"), tag("Foo", "MyEnum")),
    );
    expect(result).toEqual({ kind: "MyEnum.Foo", value: "bar" });
  });

  it("extractPrefix on Result.Ok -> {kind: 'Result', value: ...}", async () => {
    const result = await runPipeline(
      pipe(
        constant({ kind: "Result.Ok", value: 42 }),
        extractPrefix(),
      ),
    );
    expect(result).toEqual({
      kind: "Result",
      value: { kind: "Result.Ok", value: 42 },
    });
  });

  it("extractPrefix on kind with no dot -> prefix is entire kind", async () => {
    const result = await runPipeline(
      pipe(
        constant({ kind: "NoDot", value: 1 }),
        extractPrefix(),
      ),
    );
    expect(result).toEqual({
      kind: "NoDot",
      value: { kind: "NoDot", value: 1 },
    });
  });

  it("branchFamily dispatches to 'Result' arm for Result.Ok input", async () => {
    const result = await runPipeline(
      pipe(
        constant({ kind: "Result.Ok", value: 42 }),
        branchFamily({
          Result: branch({ Ok: identity(), Err: identity() }),
          Option: branch({ Some: identity(), None: identity() }),
        }),
      ),
    );
    expect(result).toBe(42);
  });

  it("branchFamily dispatches to 'Option' arm for Option.Some input", async () => {
    const result = await runPipeline(
      pipe(
        constant({ kind: "Option.Some", value: "hello" }),
        branchFamily({
          Result: branch({ Ok: identity(), Err: identity() }),
          Option: branch({ Some: identity(), None: identity() }),
        }),
      ),
    );
    expect(result).toBe("hello");
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
