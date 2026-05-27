import { describe, expect, it } from "vitest";
import { pipe } from "../src/ast.js";
import { constant, drop, identity, panic } from "../src/builtins/index.js";
import { runPipeline } from "../src/run.js";
import { setup } from "./handlers.js";
import { assertIO } from "./type-utils.js";

// ---------------------------------------------------------------------------
// Type tests
// ---------------------------------------------------------------------------

describe("scalar type tests", () => {
  it("constant: any -> T", () => {
    const action = constant({ x: 1 });
    assertIO<typeof action, any, { x: number }>();
    expect(action.kind).toBe("Invoke");
  });

  it("identity: T -> T", () => {
    const action = identity<{ x: number }>();
    assertIO<typeof action, { x: number }, { x: number }>();
    expect(action.kind).toBe("Invoke");
  });

  it("drop: any -> null", () => {
    assertIO<typeof drop, any, null>();
    expect(drop.kind).toBe("Invoke");
  });

  it("panic: any -> never", () => {
    const action = panic("boom");
    assertIO<typeof action, any, never>();
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
