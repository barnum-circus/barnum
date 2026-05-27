import { describe, expect, it } from "vitest";
import {
  type ExtractInput,
  type ExtractOutput,
  config,
  forEach,
  pipe,
} from "../src/ast.js";
import { constant, getField, wrapInField } from "../src/builtins/index.js";
import { runPipeline } from "../src/run.js";
import { listFiles, migrate, setup, verify } from "./handlers.js";
import { type IsExact, assertExact } from "./type-utils.js";

// ---------------------------------------------------------------------------
// Type tests
// ---------------------------------------------------------------------------

describe("forEach type tests", () => {
  it("forEach: wraps input/output in arrays", () => {
    const action = forEach(verify);
    assertExact<
      IsExact<ExtractInput<typeof action>, Array<{ artifact: string }>>
    >();
    assertExact<
      IsExact<ExtractOutput<typeof action>, Array<{ verified: boolean }>>
    >();
    expect(action.kind).toBe("ForEach");
  });
});

// ---------------------------------------------------------------------------
// AST structure tests
// ---------------------------------------------------------------------------

describe("forEach AST structure", () => {
  it("forEach produces ForEach AST", () => {
    const workflow = forEach(verify);
    expect(workflow.kind).toBe("ForEach");
  });

  it("forEach composes with pipe: setup → listFiles → forEach(migrate)", () => {
    const cfg = config(
      pipe(constant({ project: "test" }), setup, listFiles, forEach(migrate)),
    );
    expect(cfg.workflow.kind).toBe("Chain");
  });
});

// ---------------------------------------------------------------------------
// Execution tests
// ---------------------------------------------------------------------------

describe("forEach execution", () => {
  it("forEach maps action over array elements", async () => {
    const result = await runPipeline(
      pipe(constant([{ x: 1 }, { x: 2 }, { x: 3 }]), forEach(getField("x"))),
    );
    expect(result).toEqual([1, 2, 3]);
  });

  it("forEach on empty array returns []", async () => {
    const result = await runPipeline(
      pipe(constant([] as Array<{ x: number }>), forEach(getField("x"))),
    );
    expect(result).toEqual([]);
  });

  it("forEach on single-element array", async () => {
    const result = await runPipeline(
      pipe(constant([{ x: 42 }]), forEach(getField("x"))),
    );
    expect(result).toEqual([42]);
  });

  it("forEach composes in pipe", async () => {
    const result = await runPipeline(
      pipe(constant([10, 20, 30]), forEach(wrapInField("n"))),
    );
    expect(result).toEqual([{ n: 10 }, { n: 20 }, { n: 30 }]);
  });

  it("forEach via pipe chains correctly", async () => {
    const result = await runPipeline(
      pipe(constant([1, 2, 3]), forEach(constant(99))),
    );
    expect(result).toEqual([99, 99, 99]);
  });
});
