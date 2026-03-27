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
  process,
  check,
  finalize,
  validate,
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

  it("process: { initialized: boolean, project: string } -> { result: string }", () => {
    const action = process();
    assertExact<
      IsExact<
        ExtractInput<typeof action>,
        { initialized: boolean; project: string }
      >
    >();
    assertExact<IsExact<ExtractOutput<typeof action>, { result: string }>>();
    expect(action.kind).toBe("Invoke");
  });

  it("check: { result: string } -> { valid: boolean }", () => {
    const action = check();
    assertExact<IsExact<ExtractInput<typeof action>, { result: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { valid: boolean }>>();
    expect(action.kind).toBe("Invoke");
  });

  it("finalize: { valid: boolean } -> { done: true }", () => {
    const action = finalize();
    assertExact<IsExact<ExtractInput<typeof action>, { valid: boolean }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { done: true }>>();
    expect(action.kind).toBe("Invoke");
  });

  it("validate: { valid: boolean } -> LoopResult<{ valid: boolean }, { done: true }>", () => {
    const action = validate();
    assertExact<IsExact<ExtractInput<typeof action>, { valid: boolean }>>();
    assertExact<
      IsExact<
        ExtractOutput<typeof action>,
        LoopResult<{ valid: boolean }, { done: true }>
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
    const action = pipe(setup(), process(), check());
    assertExact<IsExact<ExtractInput<typeof action>, { project: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { valid: boolean }>>();
    expect(action.kind).toBe("Pipe");
  });

  it("forEach: wraps input/output in arrays", () => {
    const action = forEach(check());
    assertExact<IsExact<ExtractInput<typeof action>, { result: string }[]>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { valid: boolean }[]>>();
    expect(action.kind).toBe("ForEach");
  });

  it("parallel: same input, tuple output", () => {
    const action = parallel(check(), check());
    assertExact<IsExact<ExtractInput<typeof action>, { result: string }>>();
    // parallel output is [Out1, Out2]
    assertExact<
      IsExact<
        ExtractOutput<typeof action>,
        [{ valid: boolean }, { valid: boolean }]
      >
    >();
    expect(action.kind).toBe("Parallel");
  });

  it("branch: input is { kind: string }, output is case union", () => {
    const action = branch({
      Yes: finalize(),
      No: finalize(),
    });
    assertExact<IsExact<ExtractInput<typeof action>, { kind: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { done: true }>>();
    expect(action.kind).toBe("Branch");
  });

  it("loop: input matches Continue type, output is Break type", () => {
    const action = loop(validate());
    assertExact<IsExact<ExtractInput<typeof action>, { valid: boolean }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { done: true }>>();
    expect(action.kind).toBe("Loop");
  });

  it("attempt: wraps output in AttemptResult", () => {
    const action = attempt(check());
    assertExact<IsExact<ExtractInput<typeof action>, { result: string }>>();
    assertExact<
      IsExact<
        ExtractOutput<typeof action>,
        AttemptResult<{ valid: boolean }>
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
    // check outputs { valid: boolean }, setup expects { project: string }
    // @ts-expect-error — output/input mismatch
    pipe(check(), setup());
  });

  it("rejects unrelated types", () => {
    // finalize outputs { done: true }, setup expects { project: string }
    // @ts-expect-error — output/input mismatch
    pipe(finalize(), setup());
  });

  it("accepts compatible types", () => {
    // setup outputs { initialized: boolean, project: string }
    // process expects { initialized: boolean, project: string }
    const action = pipe(setup(), process());
    expect(action.kind).toBe("Pipe");
  });
});

// ---------------------------------------------------------------------------
// Config entry point
// ---------------------------------------------------------------------------

describe("config entry point", () => {
  it("rejects workflows that expect input", () => {
    // check expects { result: string } input — can't be a workflow entry point
    // @ts-expect-error — workflow entry point must accept never input
    configBuilder().workflow(() => check());
  });

  it("accepts workflows starting with constant", () => {
    const cfg = configBuilder().workflow(() =>
      pipe(constant({ result: "test" }), check()),
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
      .registerSteps({ Finalize: finalize() })
      .workflow(({ steps }) => {
        // @ts-expect-error — "Nonexistent" was never registered
        return steps.Nonexistent;
      });
  });

  it("preserves step types from static registration", () => {
    const builder = configBuilder().registerSteps({
      Check: check(),
      Finalize: finalize(),
    });

    // Verify the step types are preserved in the builder's generic
    builder.workflow(({ steps }) => {
      assertExact<
        IsExact<ExtractInput<typeof steps.Check>, { result: string }>
      >();
      assertExact<
        IsExact<ExtractOutput<typeof steps.Check>, { valid: boolean }>
      >();
      assertExact<
        IsExact<ExtractInput<typeof steps.Finalize>, { valid: boolean }>
      >();
      assertExact<IsExact<ExtractOutput<typeof steps.Finalize>, { done: true }>>();
      return pipe(constant({ result: "test" }), steps.Check, steps.Finalize);
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
      // @ts-expect-error — check outputs { valid: boolean } but self expects never
      pipe(constant({ result: "test" }), check(), self),
    );
  });

  it("preserves step types through callback form registerSteps", () => {
    configBuilder()
      .registerSteps(({ stepRef }) => ({
        A: pipe(check(), stepRef("B")),
        B: pipe(check(), stepRef("A")),
      }))
      .workflow(({ steps }) => {
        // Input type comes from check()'s input: { result: string }
        assertExact<
          IsExact<ExtractInput<typeof steps.A>, { result: string }>
        >();
        assertExact<
          IsExact<ExtractInput<typeof steps.B>, { result: string }>
        >();
        // Output is any because stepRef doesn't track output types
        assertExact<IsExact<ExtractOutput<typeof steps.A>, any>>();
        assertExact<IsExact<ExtractOutput<typeof steps.B>, any>>();
        return pipe(constant({ result: "test" }), steps.A);
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
        return { Pipeline: pipe(steps.Setup, process()) };
      });
  });

  it("preserves types across mixed object + callback batches into workflow", () => {
    configBuilder()
      // Batch 1: object form
      .registerSteps({ Setup: setup() })
      // Batch 2: callback form
      .registerSteps(({ steps }) => ({
        Pipeline: pipe(steps.Setup, process()),
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
        // pipe(steps.Setup, process()) where steps.Setup is
        // TypedAction<{ project: string }, { initialized: boolean, project: string }>
        // and process() is TypedAction<{ initialized: boolean, project: string }, { result: string }>
        assertExact<
          IsExact<ExtractInput<typeof steps.Pipeline>, { project: string }>
        >();
        assertExact<
          IsExact<ExtractOutput<typeof steps.Pipeline>, { result: string }>
        >();
        return pipe(constant({ project: "test" }), steps.Pipeline);
      });
  });
});
