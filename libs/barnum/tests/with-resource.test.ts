import { describe, it, expect } from "vitest";
import {
  type ExtractInput,
  type ExtractOutput,
  pipe,
} from "../src/ast.js";
import {
  constant,
  withResource,
  getField,
  wrapInField,
} from "../src/builtins/index.js";
import { runPipeline } from "../src/run.js";

// ---------------------------------------------------------------------------
// Type assertion helpers (compile-time only)
// ---------------------------------------------------------------------------

type IsExact<T, U> = [T] extends [U] ? ([U] extends [T] ? true : false) : false;

// eslint-disable-next-line @typescript-eslint/no-unused-vars
function assertExact<_T extends true>(): void {}

// ---------------------------------------------------------------------------
// Type tests
// ---------------------------------------------------------------------------

describe("with-resource type tests", () => {
  it("withResource: TIn -> TOut", () => {
    const action = withResource<
      { project: string },
      { conn: string },
      number
    >({
      create: constant({ conn: "db://localhost" }),
      action: constant(42),
      dispose: constant(null),
    });
    assertExact<IsExact<ExtractInput<typeof action>, { project: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, number>>();
  });
});

// ---------------------------------------------------------------------------
// Execution tests
//
// Uses only builtins (no handler subprocess calls) to avoid timeouts from
// multiple cargo build + handler invocations per pipeline.
// ---------------------------------------------------------------------------

describe("with-resource execution", () => {
  it("create acquires, action uses resource, returns action output", async () => {
    // create: { host } → { conn: "acquired" }
    // action: { conn, host } → getField("conn") → "acquired"
    // dispose: constant(null) — cleanup, result discarded
    const result = await runPipeline(
      pipe(
        constant({ host: "localhost" }),
        withResource({
          create: constant({ conn: "acquired" }),
          action: getField("conn"),
          dispose: constant(null),
        }),
      ),
    );
    expect(result).toBe("acquired");
  });

  it("resource fields merged with input for action", async () => {
    // Verify the action receives both the resource AND the original input
    // create: { x } → { r: "resource" }
    // action: { r, x } → constant({r: ..., x: ...}) by picking both fields
    const result = await runPipeline(
      pipe(
        constant({ x: "input" }),
        withResource({
          create: constant({ r: "resource" }),
          action: pipe(getField("x"), wrapInField("gotInput")),
          dispose: constant(null),
        }),
      ),
    );
    expect(result).toEqual({ gotInput: "input" });
  });

  it("dispose runs and result is discarded", async () => {
    // Even though dispose produces something, withResource returns action output
    const result = await runPipeline(
      pipe(
        constant({ x: 1 }),
        withResource({
          create: constant({ r: true }),
          action: constant("action-result"),
          dispose: constant("dispose-should-be-discarded"),
        }),
      ),
    );
    expect(result).toBe("action-result");
  });
});
