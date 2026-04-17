import { describe, it, expect } from "vitest";
import {
  type ExtractInput,
  type ExtractOutput,
  pipe,
} from "../src/ast.js";
import { constant, identity, drop, panic } from "../src/builtins/index.js";
import { runPipeline } from "../src/run.js";
import { setup } from "./handlers.js";

// ---------------------------------------------------------------------------
// Type assertion helpers (compile-time only)
// ---------------------------------------------------------------------------

type IsExact<T, U> = [T] extends [U] ? ([U] extends [T] ? true : false) : false;

// eslint-disable-next-line @typescript-eslint/no-unused-vars
function assertExact<_T extends true>(): void {}

// ---------------------------------------------------------------------------
// Type tests
// ---------------------------------------------------------------------------

describe("scalar type tests", () => {
  it("constant: any -> T", () => {
    const action = constant({ x: 1 });
    assertExact<IsExact<ExtractInput<typeof action>, any>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { x: number }>>();
    expect(action.kind).toBe("Invoke");
  });

  it("identity: T -> T", () => {
    const action = identity<{ x: number }>();
    assertExact<IsExact<ExtractInput<typeof action>, { x: number }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { x: number }>>();
    expect(action.kind).toBe("Invoke");
  });

  it("drop: any -> void", () => {
    assertExact<IsExact<ExtractInput<typeof drop>, any>>();
    assertExact<IsExact<ExtractOutput<typeof drop>, void>>();
    expect(drop.kind).toBe("Invoke");
  });

  it("panic: any -> never", () => {
    const action = panic("boom");
    assertExact<IsExact<ExtractInput<typeof action>, any>>();
    assertExact<IsExact<ExtractOutput<typeof action>, never>>();
    expect(action.kind).toBe("Invoke");
  });
});

// ---------------------------------------------------------------------------
// AST structure tests
// ---------------------------------------------------------------------------

describe("scalar AST structure", () => {
  it(".drop() produces Chain -> Drop AST", () => {
    const action = setup.drop();
    expect(action.kind).toBe("Chain");
    const chain = action as { kind: "Chain"; first: any; rest: any };
    expect(chain.first.kind).toBe("Invoke");
    expect(chain.rest.kind).toBe("Invoke");
    expect(chain.rest.handler.builtin.kind).toBe("Drop");
  });
});

// ---------------------------------------------------------------------------
// Execution tests
// ---------------------------------------------------------------------------

describe("scalar execution", () => {
  it("constant(42) returns 42", async () => {
    const result = await runPipeline(constant(42));
    expect(result).toBe(42);
  });

  it("constant('hello') returns 'hello'", async () => {
    const result = await runPipeline(constant("hello"));
    expect(result).toBe("hello");
  });

  it("constant({x: 1, y: [2, 3]}) returns object", async () => {
    const result = await runPipeline(constant({ x: 1, y: [2, 3] }));
    expect(result).toEqual({ x: 1, y: [2, 3] });
  });

  it("constant(null) returns null", async () => {
    const result = await runPipeline(constant(null));
    expect(result).toBeNull();
  });

  it("identity passes through input", async () => {
    const result = await runPipeline(identity(), { data: "passthrough" });
    expect(result).toEqual({ data: "passthrough" });
  });

  it("drop returns null", async () => {
    const result = await runPipeline(pipe(constant("discard me"), drop));
    expect(result).toBeNull();
  });

  it("panic causes runPipeline to reject", async () => {
    await expect(
      runPipeline(pipe(constant("trigger"), panic("test panic"))),
    ).rejects.toThrow();
  });
});
