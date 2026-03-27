/**
 * Pure type-level tests. Every test here is a compile-time assertion —
 * if it type-checks, the test passes. Runtime assertions are minimal.
 */
import { describe, it, expect } from "vitest";
import {
  type TypedAction,
  type ExtractInput,
  type ExtractOutput,
  type LoopResult,
  type AttemptResult,
  pipe,
  parallel,
  forEach,
  branch,
  loop,
  attempt,
  configBuilder,
} from "../src/ast.js";
import {
  constant,
  identity,
  drop,
  merge,
  flatten,
  extractField,
  range,
} from "../src/builtins.js";
import {
  setup,
  build,
  verify,
  deploy,
  healthCheck,
  listFiles,
  migrate,
  typeCheck,
  classifyErrors,
  fix,
} from "./handlers.js";

// ---------------------------------------------------------------------------
// Type assertion helpers (compile-time only)
// ---------------------------------------------------------------------------

/**
 * True when T and U are structurally identical.
 * Uses the double-extends trick to avoid distributive pitfalls.
 */
type IsExact<T, U> = [T] extends [U] ? ([U] extends [T] ? true : false) : false;

/** Compile-time assertion. Fails if T is not `true`. */
// eslint-disable-next-line @typescript-eslint/no-unused-vars
function assertExact<_T extends true>(): void {}

// ---------------------------------------------------------------------------
// Handler input/output types
// ---------------------------------------------------------------------------

describe("handler types", () => {
  it("setup: { project: string } -> { initialized: boolean, project: string }", () => {
    const action = setup();
    assertExact<IsExact<ExtractInput<typeof action>, { project: string }>>();
    assertExact<
      IsExact<
        ExtractOutput<typeof action>,
        { initialized: boolean; project: string }
      >
    >();
    expect(action.kind).toBe("Invoke");
  });

  it("build: { initialized: boolean, project: string } -> { artifact: string }", () => {
    const action = build();
    assertExact<
      IsExact<
        ExtractInput<typeof action>,
        { initialized: boolean; project: string }
      >
    >();
    assertExact<IsExact<ExtractOutput<typeof action>, { artifact: string }>>();
    expect(action.kind).toBe("Invoke");
  });

  it("verify: { artifact: string } -> { verified: boolean }", () => {
    const action = verify();
    assertExact<IsExact<ExtractInput<typeof action>, { artifact: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { verified: boolean }>>();
    expect(action.kind).toBe("Invoke");
  });

  it("deploy: { verified: boolean } -> { deployed: true }", () => {
    const action = deploy();
    assertExact<IsExact<ExtractInput<typeof action>, { verified: boolean }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { deployed: true }>>();
    expect(action.kind).toBe("Invoke");
  });

  it("healthCheck: { deployed: boolean } -> LoopResult<{ deployed: boolean }, { stable: true }>", () => {
    const action = healthCheck();
    assertExact<IsExact<ExtractInput<typeof action>, { deployed: boolean }>>();
    assertExact<
      IsExact<
        ExtractOutput<typeof action>,
        LoopResult<{ deployed: boolean }, { stable: true }>
      >
    >();
    expect(action.kind).toBe("Invoke");
  });

  it("listFiles: { initialized: boolean, project: string } -> { file: string }[]", () => {
    const action = listFiles();
    assertExact<
      IsExact<
        ExtractInput<typeof action>,
        { initialized: boolean; project: string }
      >
    >();
    assertExact<IsExact<ExtractOutput<typeof action>, { file: string }[]>>();
    expect(action.kind).toBe("Invoke");
  });

  it("migrate: { file: string } -> { file: string, migrated: boolean }", () => {
    const action = migrate();
    assertExact<IsExact<ExtractInput<typeof action>, { file: string }>>();
    assertExact<
      IsExact<
        ExtractOutput<typeof action>,
        { file: string; migrated: boolean }
      >
    >();
    expect(action.kind).toBe("Invoke");
  });

  it("typeCheck: never -> TypeError[]", () => {
    const action = typeCheck();
    assertExact<IsExact<ExtractInput<typeof action>, never>>();
    assertExact<
      IsExact<
        ExtractOutput<typeof action>,
        { file: string; message: string }[]
      >
    >();
    expect(action.kind).toBe("Invoke");
  });

  it("fix: { file: string, message: string } -> { file: string, fixed: boolean }", () => {
    const action = fix();
    assertExact<
      IsExact<
        ExtractInput<typeof action>,
        { file: string; message: string }
      >
    >();
    assertExact<
      IsExact<
        ExtractOutput<typeof action>,
        { file: string; fixed: boolean }
      >
    >();
    expect(action.kind).toBe("Invoke");
  });
});

// ---------------------------------------------------------------------------
// Builtin types
// ---------------------------------------------------------------------------

describe("builtin types", () => {
  it("constant: never -> T", () => {
    const action = constant({ x: 1 });
    assertExact<IsExact<ExtractInput<typeof action>, never>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { x: number }>>();
    expect(action.kind).toBe("Invoke");
  });

  it("identity: T -> T", () => {
    const action = identity<{ x: number }>();
    assertExact<IsExact<ExtractInput<typeof action>, { x: number }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { x: number }>>();
    expect(action.kind).toBe("Invoke");
  });

  it("drop: T -> never", () => {
    const action = drop<string>();
    assertExact<IsExact<ExtractInput<typeof action>, string>>();
    assertExact<IsExact<ExtractOutput<typeof action>, never>>();
    expect(action.kind).toBe("Invoke");
  });

  it("range: never -> number[]", () => {
    const action = range(0, 10);
    assertExact<IsExact<ExtractInput<typeof action>, never>>();
    assertExact<IsExact<ExtractOutput<typeof action>, number[]>>();
    expect(action.kind).toBe("Invoke");
  });

  it("flatten: T[][] -> T[]", () => {
    const action = flatten<number>();
    assertExact<IsExact<ExtractInput<typeof action>, number[][]>>();
    assertExact<IsExact<ExtractOutput<typeof action>, number[]>>();
    expect(action.kind).toBe("Invoke");
  });

  it("extractField: { key: V } -> V", () => {
    const action = extractField<{ name: string; age: number }, "name">("name");
    assertExact<
      IsExact<ExtractInput<typeof action>, { name: string; age: number }>
    >();
    assertExact<IsExact<ExtractOutput<typeof action>, string>>();
    expect(action.kind).toBe("Invoke");
  });

  it("merge: [A, B] -> A & B", () => {
    const action = merge<[{ a: number }, { b: string }]>();
    assertExact<
      IsExact<ExtractInput<typeof action>, [{ a: number }, { b: string }]>
    >();
    assertExact<
      IsExact<ExtractOutput<typeof action>, { a: number } & { b: string }>
    >();
    expect(action.kind).toBe("Invoke");
  });
});

// ---------------------------------------------------------------------------
// Combinator types
// ---------------------------------------------------------------------------

describe("combinator types", () => {
  it("pipe: input of first, output of last", () => {
    const action = pipe(setup(), build(), verify());
    assertExact<IsExact<ExtractInput<typeof action>, { project: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { verified: boolean }>>();
    expect(action.kind).toBe("Pipe");
  });

  it("forEach: wraps input/output in arrays", () => {
    const action = forEach(verify());
    assertExact<IsExact<ExtractInput<typeof action>, { artifact: string }[]>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { verified: boolean }[]>>();
    expect(action.kind).toBe("ForEach");
  });

  it("parallel: same input, tuple output", () => {
    const action = parallel(verify(), verify());
    assertExact<IsExact<ExtractInput<typeof action>, { artifact: string }>>();
    // parallel output is [Out1, Out2]
    assertExact<
      IsExact<
        ExtractOutput<typeof action>,
        [{ verified: boolean }, { verified: boolean }]
      >
    >();
    expect(action.kind).toBe("Parallel");
  });

  it("branch: input is { kind: string }, output is case union", () => {
    const action = branch({
      Yes: deploy(),
      No: deploy(),
    });
    assertExact<IsExact<ExtractInput<typeof action>, { kind: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { deployed: true }>>();
    expect(action.kind).toBe("Branch");
  });

  it("loop: input matches Continue type, output is Break type", () => {
    const action = loop(healthCheck());
    assertExact<IsExact<ExtractInput<typeof action>, { deployed: boolean }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { stable: true }>>();
    expect(action.kind).toBe("Loop");
  });

  it("attempt: wraps output in AttemptResult", () => {
    const action = attempt(verify());
    assertExact<IsExact<ExtractInput<typeof action>, { artifact: string }>>();
    assertExact<
      IsExact<
        ExtractOutput<typeof action>,
        AttemptResult<{ verified: boolean }>
      >
    >();
    expect(action.kind).toBe("Attempt");
  });
});

// ---------------------------------------------------------------------------
// Pipe type errors
// ---------------------------------------------------------------------------

describe("pipe type safety", () => {
  it("rejects mismatched adjacent types", () => {
    // verify outputs { verified: boolean }, setup expects { project: string }
    // @ts-expect-error — output/input mismatch
    pipe(verify(), setup());
  });

  it("rejects unrelated types", () => {
    // deploy outputs { deployed: true }, setup expects { project: string }
    // @ts-expect-error — output/input mismatch
    pipe(deploy(), setup());
  });

  it("accepts compatible types", () => {
    // setup outputs { initialized: boolean, project: string }
    // build expects { initialized: boolean, project: string }
    const action = pipe(setup(), build());
    expect(action.kind).toBe("Pipe");
  });
});

// ---------------------------------------------------------------------------
// Config entry point
// ---------------------------------------------------------------------------

describe("config entry point", () => {
  it("rejects workflows that expect input", () => {
    // verify expects { artifact: string } input — can't be a workflow entry point
    // @ts-expect-error — workflow entry point must accept never input
    configBuilder().workflow(() => verify());
  });

  it("accepts workflows starting with constant", () => {
    const cfg = configBuilder().workflow(() =>
      pipe(constant({ artifact: "test" }), verify()),
    );
    expect(cfg.workflow.kind).toBe("Pipe");
  });
});

// ---------------------------------------------------------------------------
// Step references
// ---------------------------------------------------------------------------

describe("step reference types", () => {
  it("rejects references to unregistered steps", () => {
    configBuilder()
      .registerSteps({ Deploy: deploy() })
      .workflow(({ steps }) => {
        // @ts-expect-error — "Nonexistent" was never registered
        return steps.Nonexistent;
      });
  });

  it("preserves step types from static registration", () => {
    const builder = configBuilder().registerSteps({
      Verify: verify(),
      Deploy: deploy(),
    });

    // Verify the step types are preserved in the builder's generic
    builder.workflow(({ steps }) => {
      assertExact<
        IsExact<ExtractInput<typeof steps.Verify>, { artifact: string }>
      >();
      assertExact<
        IsExact<ExtractOutput<typeof steps.Verify>, { verified: boolean }>
      >();
      assertExact<
        IsExact<ExtractInput<typeof steps.Deploy>, { verified: boolean }>
      >();
      assertExact<IsExact<ExtractOutput<typeof steps.Deploy>, { deployed: true }>>();
      return pipe(constant({ artifact: "test" }), steps.Verify, steps.Deploy);
    });
  });

  it("self is TypedAction<never, never>", () => {
    configBuilder().workflow(({ self }) => {
      assertExact<IsExact<ExtractInput<typeof self>, never>>();
      assertExact<IsExact<ExtractOutput<typeof self>, never>>();
      return constant({ done: true });
    });
  });

  it("self cannot be piped after a value-producing action", () => {
    configBuilder().workflow(({ self }) =>
      // @ts-expect-error — verify outputs { verified: boolean } but self expects never
      pipe(constant({ artifact: "test" }), verify(), self),
    );
  });

  it("preserves step types through callback form registerSteps", () => {
    configBuilder()
      .registerSteps(({ stepRef }) => ({
        A: pipe(verify(), stepRef("B")),
        B: pipe(verify(), stepRef("A")),
      }))
      .workflow(({ steps }) => {
        // Input type comes from verify()'s input: { artifact: string }
        assertExact<
          IsExact<ExtractInput<typeof steps.A>, { artifact: string }>
        >();
        assertExact<
          IsExact<ExtractInput<typeof steps.B>, { artifact: string }>
        >();
        // Output is any because stepRef doesn't track output types
        assertExact<IsExact<ExtractOutput<typeof steps.A>, any>>();
        assertExact<IsExact<ExtractOutput<typeof steps.B>, any>>();
        return pipe(constant({ artifact: "test" }), steps.A);
      });
  });

  it("callback steps parameter excludes current-batch keys", () => {
    configBuilder()
      .registerSteps({ Setup: setup() })
      .registerSteps(({ steps }) => {
        // steps.Setup exists (from prior batch)
        assertExact<
          IsExact<ExtractInput<typeof steps.Setup>, { project: string }>
        >();
        // @ts-expect-error — Pipeline is in the current batch, not prior
        steps.Pipeline;
        return { Pipeline: pipe(steps.Setup, build()) };
      });
  });

  it("preserves types across mixed object + callback batches into workflow", () => {
    configBuilder()
      // Batch 1: object form
      .registerSteps({ Setup: setup() })
      // Batch 2: callback form
      .registerSteps(({ steps }) => ({
        Pipeline: pipe(steps.Setup, build()),
      }))
      .workflow(({ steps }) => {
        // Batch 1 step types survive
        assertExact<
          IsExact<ExtractInput<typeof steps.Setup>, { project: string }>
        >();
        assertExact<
          IsExact<
            ExtractOutput<typeof steps.Setup>,
            { initialized: boolean; project: string }
          >
        >();
        // Batch 2 step types survive — input comes from steps.Setup (a Step
        // ref at runtime), but the static type is what registerSteps inferred:
        // pipe(steps.Setup, build()) where steps.Setup is
        // TypedAction<{ project: string }, { initialized: boolean, project: string }>
        // and build() is TypedAction<{ initialized: boolean, project: string }, { artifact: string }>
        assertExact<
          IsExact<ExtractInput<typeof steps.Pipeline>, { project: string }>
        >();
        assertExact<
          IsExact<ExtractOutput<typeof steps.Pipeline>, { artifact: string }>
        >();
        return pipe(constant({ project: "test" }), steps.Pipeline);
      });
  });
});
