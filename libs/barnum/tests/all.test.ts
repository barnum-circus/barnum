import { describe, expect, it } from "vitest";
import { all, config, pipe } from "../src/ast.js";
import { constant, getField, identity } from "../src/builtins/index.js";
import { runPipeline } from "../src/run.js";
import { build, setup, verify } from "./handlers.js";
import { type CheckIO, assertExact } from "./type-utils.js";

// ---------------------------------------------------------------------------
// Type tests
// ---------------------------------------------------------------------------

describe("all type tests", () => {
  it("all: same input, tuple output", () => {
    const action = all(verify, verify);
    assertExact<
      CheckIO<
        typeof action,
        { artifact: string },
        [{ verified: boolean }, { verified: boolean }]
      >
    >();
    expect(action.kind).toBe("All");
  });
});

// ---------------------------------------------------------------------------
// AST structure tests
// ---------------------------------------------------------------------------

describe("all AST structure", () => {
  it("all accepts actions with the same input type", () => {
    const workflow = all(verify, verify);
    expect(workflow.kind).toBe("All");
  });

  it("rejects actions with different input types", () => {
    // setup expects { project: string }, verify expects { artifact: string }
    // @ts-expect-error — input types do not unify
    all(setup, verify);
  });

  it("all composes with pipe", () => {
    const cfg = config(
      pipe(constant({ project: "test" }), all(setup, pipe(setup, build))),
    );
    expect(cfg.workflow.kind).toBe("Chain");
  });
});

// ---------------------------------------------------------------------------
// Execution tests
// ---------------------------------------------------------------------------

describe("all execution", () => {
  it("all runs actions, returns tuple of results", async () => {
    const result = await runPipeline(
      pipe(constant({ x: 10, y: 20 }), all(getField("x"), getField("y"))),
    );
    expect(result).toEqual([10, 20]);
  });

  it("all with identity preserves input alongside other action", async () => {
    const result = await runPipeline(
      pipe(constant({ x: 42 }), all(identity(), getField("x"))),
    );
    expect(result).toEqual([{ x: 42 }, 42]);
  });

  it("all with 3 actions returns 3-tuple", async () => {
    const result = await runPipeline(
      pipe(
        constant({ a: 1, b: 2, c: 3 }),
        all(getField("a"), getField("b"), getField("c")),
      ),
    );
    expect(result).toEqual([1, 2, 3]);
  });

  it("all with constant actions (independent of input)", async () => {
    const result = await runPipeline(all(constant("hello"), constant(42)));
    expect(result).toEqual(["hello", 42]);
  });
});
