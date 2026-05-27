import { describe, expect, it } from "vitest";
import { all, pipe } from "../src/ast.js";
import { constant, withResource } from "../src/builtins/index.js";
import { runPipeline } from "../src/run.js";
import { type CheckIO, assertExact } from "./type-utils.js";

// ---------------------------------------------------------------------------
// Type tests
// ---------------------------------------------------------------------------

describe("withResource type tests", () => {
  it("withResource has the correct type", () => {
    const action = withResource<
      { input: string },
      { resource: string },
      { output: string }
    >({
      create: constant({ resource: "res" }),
      action: () => constant({ output: "output" }),
      dispose: constant(null),
    });

    assertExact<
      CheckIO<typeof action, { input: string }, { output: string }>
    >();
  });
  it("varRefs passed to withResource have the correct types", () => {
    withResource<{ input: string }, { resource: string }, { output: string }>({
      create: constant({ resource: "res" }),
      action: (resourceRef, inputRef) => {
        assertExact<CheckIO<typeof resourceRef, any, { resource: string }>>();
        assertExact<CheckIO<typeof inputRef, any, { input: string }>>();

        return constant({ output: "output" });
      },
      dispose: constant(null),
    });
  });
});

// ---------------------------------------------------------------------------
// Execution tests
//
// Uses only builtins (no handler subprocess calls) to avoid timeouts from
// multiple cargo build + handler invocations per pipeline.
// ---------------------------------------------------------------------------

describe("withResource execution", () => {
  it("create acquires, action uses resource, returns action output", async () => {
    // action receives [resource, input] tuple; constant ignores input
    const result = await runPipeline(
      pipe(
        constant({ host: "localhost" }),
        withResource({
          create: constant({ conn: "acquired" }),
          action: () => constant("action-output"),
          dispose: constant(null),
        }),
      ),
    );
    expect(result).toBe("action-output");
  });

  it("action receives [resource, input] tuple", async () => {
    // identity() passes the tuple through so we can assert its shape
    const result = await runPipeline(
      pipe(
        constant({ input: "input" }),
        withResource({
          create: constant({ resource: "resource" }),
          action: (resource, input) => all(resource, input),
          dispose: constant(null),
        }),
      ),
    );
    expect(result).toEqual([{ resource: "resource" }, { input: "input" }]);
  });

  it("dispose runs and result is discarded", async () => {
    // Even though dispose produces something, withResource returns action output
    const result = await runPipeline(
      pipe(
        constant({ x: 1 }),
        withResource({
          create: constant({ r: true }),
          action: () => constant("action-result"),
          dispose: constant("dispose-should-be-discarded"),
        }),
      ),
    );
    expect(result).toBe("action-result");
  });
});
