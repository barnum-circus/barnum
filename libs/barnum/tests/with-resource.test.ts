import { describe, expect, it } from "vitest";
import { bindInput, pipe } from "../src/ast.js";
import { constant, identity, withResource } from "../src/builtins/index.js";
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
      action: constant({ output: "output" }),
      dispose: constant(null),
    });

    assertExact<
      CheckIO<typeof action, { input: string }, { output: string }>
    >();
  });

  it("action receives [resource, input] as input type", () => {
    withResource<{ input: string }, { resource: string }, { output: string }>({
      create: constant({ resource: "res" }),
      action: bindInput<
        [{ resource: string }, { input: string }],
        { output: string }
      >((state) => {
        const [resource, input] = state.split();
        assertExact<CheckIO<typeof resource, any, { resource: string }>>();
        assertExact<CheckIO<typeof input, any, { input: string }>>();

        return resource.then(constant({ output: "output" }));
      }),
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
    const result = await runPipeline(
      pipe(
        constant({ host: "localhost" }),
        withResource({
          create: constant({ conn: "acquired" }),
          action: constant("action-output"),
          dispose: constant(null),
        }),
      ),
    );
    expect(result).toBe("action-output");
  });

  it("action receives [resource, input] tuple", async () => {
    const result = await runPipeline(
      pipe(
        constant({ input: "input" }),
        withResource({
          create: constant({ resource: "resource" }),
          action: identity(),
          dispose: constant(null),
        }),
      ),
    );
    expect(result).toEqual([{ resource: "resource" }, { input: "input" }]);
  });

  it("action can use split to destructure", async () => {
    const result = await runPipeline(
      pipe(
        constant({ input: "hello" }),
        withResource({
          create: constant({ resource: "world" }),
          action: bindInput<[{ resource: string }, { input: string }], string>(
            (state) => {
              const [resource, _input] = state.split();
              return resource.getField("resource");
            },
          ),
          dispose: constant(null),
        }),
      ),
    );
    expect(result).toBe("world");
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
