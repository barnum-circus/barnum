/**
 * Pure type-level tests. Every test here is a compile-time assertion —
 * if it type-checks, the test passes. Runtime assertions are minimal.
 */
import { describe, it, expect, beforeEach } from "vitest";
import {
  type TaggedUnion,
  type TypedAction,
  type ExtractInput,
  type ExtractOutput,
  type ExtractRefs,
  type LoopResult,
  type Option,
  type OptionDef,
  type Result,
  type ResultDef,
  type VarRef,
  pipe,
  all,
  forEach,
  branch,
  loop,
  workflowBuilder,
  bind,
  bindInput,
  resetEffectIdCounter,
} from "../src/ast.js";
import {
  constant,
  identity,
  drop,
  merge,
  flatten,
  extractField,
  extractIndex,
  range,
  recur,
  done,
  tag,
  Option as O,
  Result as R,
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
  type TypeError,
  type ClassifyResult,
} from "./handlers.js";

type HasErrors = Extract<ClassifyResult, { kind: "HasErrors" }>;
type Clean = Extract<ClassifyResult, { kind: "Clean" }>;

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
    const action = setup;
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
    const action = build;
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
    const action = verify;
    assertExact<IsExact<ExtractInput<typeof action>, { artifact: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { verified: boolean }>>();
    expect(action.kind).toBe("Invoke");
  });

  it("deploy: { verified: boolean } -> { deployed: boolean }", () => {
    const action = deploy;
    assertExact<IsExact<ExtractInput<typeof action>, { verified: boolean }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { deployed: boolean }>>();
    expect(action.kind).toBe("Invoke");
  });

  it("healthCheck: { deployed: boolean } -> LoopResult<{ deployed: boolean }, { stable: true }>", () => {
    const action = healthCheck;
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
    const action = listFiles;
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
    const action = migrate;
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
    const action = typeCheck;
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
    const action = fix;
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
    const action = pipe(setup, build, verify);
    assertExact<IsExact<ExtractInput<typeof action>, { project: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { verified: boolean }>>();
    expect(action.kind).toBe("Chain");
  });

  it("forEach: wraps input/output in arrays", () => {
    const action = forEach(verify);
    assertExact<IsExact<ExtractInput<typeof action>, { artifact: string }[]>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { verified: boolean }[]>>();
    expect(action.kind).toBe("ForEach");
  });

  it("all: same input, tuple output", () => {
    const action = all(verify, verify);
    assertExact<IsExact<ExtractInput<typeof action>, { artifact: string }>>();
    // all output is [Out1, Out2]
    assertExact<
      IsExact<
        ExtractOutput<typeof action>,
        [{ verified: boolean }, { verified: boolean }]
      >
    >();
    expect(action.kind).toBe("All");
  });

  it("branch: input is discriminated union with kind, output is case union", () => {
    const action = branch({
      Yes: deploy,
      No: deploy,
    });
    // BranchInput wraps handler input in { kind: K; value: T }
    assertExact<
      IsExact<
        ExtractInput<typeof action>,
        | { kind: "Yes"; value: { verified: boolean } }
        | { kind: "No"; value: { verified: boolean } }
      >
    >();
    assertExact<IsExact<ExtractOutput<typeof action>, { deployed: boolean }>>();
    expect(action.kind).toBe("Branch");
  });

  it("loop: input matches Continue type, output is Break type", () => {
    const action = loop(healthCheck);
    assertExact<IsExact<ExtractInput<typeof action>, { deployed: boolean }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { stable: true }>>();
    expect(action.kind).toBe("Loop");
  });

  it("branch with recur/done: output is union of Continue and Break members", () => {
    const action = pipe(
      classifyErrors,
      branch({
        HasErrors: pipe(forEach(fix), recur<any, void>()),
        Clean: done<any, void>(),
      }),
    );
    assertExact<
      IsExact<ExtractInput<typeof action>, TypeError[]>
    >();
    // Branch output is the union of recur's Continue and done's Break.
    // recur<any, void>() uses `any` as the invariance escape hatch for loop bodies.
    assertExact<
      IsExact<
        ExtractOutput<typeof action>,
        | { kind: "Continue"; value: any }
        | { kind: "Break"; value: void }
      >
    >();
    expect(action.kind).toBe("Chain");
  });

  it("loop with branch/recur/done: output is Break value type", () => {
    const action = loop(
      pipe(
        drop<any>(),
        typeCheck,
        classifyErrors,
        branch({
          HasErrors: pipe(forEach(fix), recur<any, void>()),
          Clean: done<any, void>(),
        }),
      ),
    );
    // Loop input: whatever drop() accepts (inferred from context)
    // Loop output: the Break value from done() in the Clean case (void after auto-unwrap)
    assertExact<
      IsExact<ExtractOutput<typeof action>, void>
    >();
    expect(action.kind).toBe("Loop");
  });

  it("full pipeline: constant → handlers → forEach → loop", () => {
    const action = pipe(
      constant({ project: "test" }),
      setup,
      listFiles,
      forEach(migrate),
      loop(
        pipe(
          drop<any>(),
          typeCheck,
          classifyErrors,
          branch({
            HasErrors: pipe(forEach(fix), recur<any, void>()),
            Clean: done<any, void>(),
          }),
        ),
      ),
    );
    assertExact<IsExact<ExtractInput<typeof action>, never>>();
    assertExact<
      IsExact<ExtractOutput<typeof action>, void>
    >();
    expect(action.kind).toBe("Chain");
  });

});

// ---------------------------------------------------------------------------
// Postfix operator types
// ---------------------------------------------------------------------------

describe("postfix operator types", () => {
  it(".branch(): input preserved, output is union of case outputs", () => {
    const action = classifyErrors.branch({
      HasErrors: forEach(fix),
      Clean: drop(),
    });
    assertExact<IsExact<ExtractInput<typeof action>, TypeError[]>>();
    // Output is union: fix[]'s output | never (from drop)
    assertExact<
      IsExact<
        ExtractOutput<typeof action>,
        { file: string; fixed: boolean }[] | never
      >
    >();
    expect(action.kind).toBe("Chain");
  });

  it(".flatten(): nested array becomes flat", () => {
    const action = forEach(forEach(verify)).flatten();
    assertExact<
      IsExact<ExtractInput<typeof action>, { artifact: string }[][]>
    >();
    assertExact<
      IsExact<ExtractOutput<typeof action>, { verified: boolean }[]>
    >();
    expect(action.kind).toBe("Chain");
  });

  it(".drop(): output is never", () => {
    const action = setup.drop();
    assertExact<IsExact<ExtractInput<typeof action>, { project: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, never>>();
    expect(action.kind).toBe("Chain");
  });

  it(".tag(): wraps output in tagged union with __def", () => {
    type Def = { Success: { verified: boolean }; Failure: string };
    const action = verify.tag<Def, "Success">("Success");
    assertExact<IsExact<ExtractInput<typeof action>, { artifact: string }>>();
    assertExact<
      IsExact<
        ExtractOutput<typeof action>,
        TaggedUnion<Def>
      >
    >();
    expect(action.kind).toBe("Chain");
  });

  it(".get(): extracts field from output", () => {
    const action = pipe(
      constant({ name: "test", age: 42 }),
    ).get("name");
    assertExact<IsExact<ExtractInput<typeof action>, never>>();
    assertExact<IsExact<ExtractOutput<typeof action>, string>>();
    expect(action.kind).toBe("Chain");
  });

  it(".get() chains with .then()", () => {
    const action = pipe(
      constant({ result: { data: [1, 2, 3] } }),
    ).get("result").get("data");
    assertExact<IsExact<ExtractOutput<typeof action>, number[]>>();
    expect(action.kind).toBe("Chain");
  });

  it("postfix .branch() + .drop() compose in chain", () => {
    const action = pipe(
      typeCheck,
      classifyErrors,
    ).branch({
      HasErrors: pipe(forEach(fix), recur<any, void>()),
      Clean: done<any, void>(),
    });
    expect(action.kind).toBe("Chain");
  });
});

// ---------------------------------------------------------------------------
// { kind, value } convention
// ---------------------------------------------------------------------------

describe("{ kind, value } convention", () => {
  it("ClassifyResult uses { kind, value } form", () => {
    assertExact<IsExact<Extract<ClassifyResult, { kind: "HasErrors" }>, { kind: "HasErrors"; value: TypeError[] }>>();
  });

  it("branch auto-unwraps: HasErrors handler receives TypeError[] directly", () => {
    classifyErrors.branch({
      HasErrors: forEach(fix),
      Clean: drop(),
    });
  });
});

// ---------------------------------------------------------------------------
// Phantom __def on tagged unions
// ---------------------------------------------------------------------------

describe("phantom __def on tagged unions", () => {
  it("ClassifyResult variants carry __def phantom field", () => {
    type Def = { HasErrors: TypeError[]; Clean: void };
    assertExact<IsExact<Extract<ClassifyResult, { kind: "HasErrors" }>, { kind: "HasErrors"; value: TypeError[]; __def?: Def }>>();
  });

  it("LoopResult variants carry __def phantom field", () => {
    type Def = { Continue: { deployed: boolean }; Break: { stable: true } };
    type LR = LoopResult<{ deployed: boolean }, { stable: true }>;
    assertExact<IsExact<Extract<LR, { kind: "Continue" }>, { kind: "Continue"; value: { deployed: boolean }; __def?: Def }>>();
  });
});

// ---------------------------------------------------------------------------
// Postfix .branch() type safety (contravariant case handlers)
// ---------------------------------------------------------------------------

describe("postfix .branch() type safety", () => {
  it("rejects non-exhaustive postfix branch", () => {
    // @ts-expect-error — non-exhaustive: missing "Clean" case
    classifyErrors.branch({ HasErrors: drop() });
  });

  it("rejects wrong handler type in postfix branch", () => {
    // @ts-expect-error — deploy expects { verified: boolean }, not HasErrors variant
    classifyErrors.branch({ HasErrors: deploy, Clean: drop() });
  });

  it("accepts exhaustive postfix branch with bare drop()", () => {
    classifyErrors.branch({
      HasErrors: drop(),
      Clean: drop(),
    });
  });

  it("rejects .branch() on non-discriminated output", () => {
    // @ts-expect-error — Out has no kind, .branch() should reject
    deploy.branch({ A: drop() });
  });

  it("accepts case handlers with step refs (non-never Refs)", () => {
    // stepRef returns TypedAction<any, any, N> — the N brand must be
    // accepted by the branch constraint (CaseHandler allows string refs).
    // This is critical for mutual recursion patterns: branch dispatches
    // to named steps via stepRef inside .branch() cases.
    workflowBuilder()
      .registerSteps(({ stepRef }) => ({
        TypeCheck: classifyErrors.branch({
          HasErrors: stepRef("Fix"),
          Clean: drop(),
        }),
        Fix: pipe(forEach(fix).drop(), stepRef("TypeCheck")),
      }));
    // Type-checks iff CaseHandler constraint accepts non-never refs.
    // Before fix: CaseHandler defaulted TRefs=never, rejecting stepRef("Fix").
  });

  it("branch collects refs from case handlers", () => {
    // Branch return type should union refs from all case handlers.
    const branched = classifyErrors.branch({
      HasErrors: drop(),
      Clean: drop(),
    });
    // With no step refs, Refs should be never
    assertExact<IsExact<ExtractRefs<typeof branched>, never>>();
  });

  it("branch propagates refs from step refs in cases", () => {
    // When case handlers carry step refs, .branch() collects them.
    workflowBuilder()
      .registerSteps(({ stepRef }) => {
        const branched = classifyErrors.branch({
          HasErrors: stepRef("Fix"),
          Clean: stepRef("Other"),
        });
        // Refs should be "Fix" | "Other"
        assertExact<IsExact<ExtractRefs<typeof branched>, "Fix" | "Other">>();
        return {
          Fix: drop() as TypedAction<any, any>,
          Other: drop() as TypedAction<any, any>,
          Root: branched,
        };
      });
  });
});

// ---------------------------------------------------------------------------
// Pipe type errors
// ---------------------------------------------------------------------------

describe("pipe type safety", () => {
  it("rejects mismatched adjacent types", () => {
    // verify outputs { verified: boolean }, setup expects { project: string }
    // @ts-expect-error — output/input mismatch
    pipe(verify, setup);
  });

  it("rejects unrelated types", () => {
    // deploy outputs { deployed: true }, setup expects { project: string }
    // @ts-expect-error — output/input mismatch
    pipe(deploy, setup);
  });

  it("accepts compatible types", () => {
    // setup outputs { initialized: boolean, project: string }
    // build expects { initialized: boolean, project: string }
    const action = pipe(setup, build);
    expect(action.kind).toBe("Chain");
  });

  it("rejects non-exhaustive branch (missing case)", () => {
    // classifyErrors outputs { kind: "HasErrors"; ... } | { kind: "Clean" }
    // branch with only HasErrors case produces { kind: "HasErrors" } input
    // pipe rejects because { kind: "Clean" } is not assignable to { kind: "HasErrors" }
    // @ts-expect-error — non-exhaustive: missing "Clean" case
    pipe(classifyErrors, branch({ HasErrors: drop() }));
  });

  it("accepts exhaustive branch", () => {
    const action = pipe(
      classifyErrors,
      branch({ HasErrors: drop<TypeError[]>(), Clean: drop<void>() }),
    );
    expect(action.kind).toBe("Chain");
  });
});

// ---------------------------------------------------------------------------
// Config entry point
// ---------------------------------------------------------------------------

describe("config entry point", () => {
  it("rejects workflows that expect input", () => {
    // verify expects { artifact: string } input — can't be a workflow entry point
    // @ts-expect-error — workflow entry point must accept never input
    workflowBuilder().workflow(() => verify);
  });

  it("accepts workflows starting with constant", () => {
    const cfg = workflowBuilder().workflow(() =>
      pipe(constant({ artifact: "test" }), verify),
    );
    expect(cfg.workflow.kind).toBe("Chain");
  });
});

// ---------------------------------------------------------------------------
// Step references
// ---------------------------------------------------------------------------

describe("step reference types", () => {
  it("rejects references to unregistered steps", () => {
    workflowBuilder()
      .registerSteps({ Deploy: deploy })
      .workflow(({ steps }) => {
        // @ts-expect-error — "Nonexistent" was never registered
        return steps.Nonexistent;
      });
  });

  it("preserves step types from static registration", () => {
    const builder = workflowBuilder().registerSteps({
      Verify: verify,
      Deploy: deploy,
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
      assertExact<IsExact<ExtractOutput<typeof steps.Deploy>, { deployed: boolean }>>();
      return pipe(constant({ artifact: "test" }), steps.Verify, steps.Deploy);
    });
  });

  it("self is TypedAction<never, never>", () => {
    workflowBuilder().workflow(({ self }) => {
      assertExact<IsExact<ExtractInput<typeof self>, never>>();
      assertExact<IsExact<ExtractOutput<typeof self>, never>>();
      return constant({ done: true });
    });
  });

  it("self cannot be piped after a value-producing action", () => {
    workflowBuilder().workflow(({ self }) =>
      // @ts-expect-error — verify outputs { verified: boolean } but self expects never
      pipe(constant({ artifact: "test" }), verify, self),
    );
  });

  it("preserves step types through callback form registerSteps", () => {
    workflowBuilder()
      .registerSteps(({ stepRef }) => ({
        A: pipe(verify, stepRef("B")),
        B: pipe(verify, stepRef("A")),
      }))
      .workflow(({ steps }) => {
        // Input type comes from verify's input: { artifact: string }
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
    workflowBuilder()
      .registerSteps({ Setup: setup })
      .registerSteps(({ steps }) => {
        // steps.Setup exists (from prior batch)
        assertExact<
          IsExact<ExtractInput<typeof steps.Setup>, { project: string }>
        >();
        // @ts-expect-error — Pipeline is in the current batch, not prior
        steps.Pipeline;
        return { Pipeline: pipe(steps.Setup, build) };
      });
  });

  it("preserves types across mixed object + callback batches into workflow", () => {
    workflowBuilder()
      // Batch 1: object form
      .registerSteps({ Setup: setup })
      // Batch 2: callback form
      .registerSteps(({ steps }) => ({
        Pipeline: pipe(steps.Setup, build),
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
        // pipe(steps.Setup, build) where steps.Setup is
        // TypedAction<{ project: string }, { initialized: boolean, project: string }>
        // and build is TypedAction<{ initialized: boolean, project: string }, { artifact: string }>
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

// ---------------------------------------------------------------------------
// Postfix .mapOption() — this-parameter constraint prototype
// ---------------------------------------------------------------------------

describe("postfix .mapOption() type safety", () => {
  it("compiles when Out is Option<T>", () => {
    // Construct an action whose output is Option<{ verified: boolean }>
    const optionAction = verify.tag<OptionDef<{ verified: boolean }>, "Some">("Some");
    // mapOption should accept a handler that transforms the Some payload
    const mapped = optionAction.mapOption(deploy);
    assertExact<IsExact<ExtractInput<typeof mapped>, { artifact: string }>>();
    assertExact<IsExact<ExtractOutput<typeof mapped>, Option<{ deployed: boolean }>>>();
    expect(mapped.kind).toBe("Chain");
  });

  it("rejects .mapOption() when Out is not Option<T>", () => {
    // verify outputs { verified: boolean } — not an Option
    // @ts-expect-error — mapOption requires Out to be Option<T>
    verify.mapOption(deploy);
  });

  it("rejects .mapOption() when Out is a different tagged union", () => {
    // classifyErrors outputs ClassifyResult = { kind: "HasErrors"; ... } | { kind: "Clean"; ... }
    // Not Option<T> (which has kind "Some" | "None")
    // @ts-expect-error — mapOption requires Out to be Option<T>
    classifyErrors.mapOption(deploy);
  });

  it("preserves input type through mapOption", () => {
    const optionAction = pipe(
      constant({ artifact: "test" }),
      verify,
      tag<OptionDef<{ verified: boolean }>, "Some">("Some"),
    );
    const mapped = optionAction.mapOption(deploy);
    assertExact<IsExact<ExtractInput<typeof mapped>, never>>();
    assertExact<IsExact<ExtractOutput<typeof mapped>, Option<{ deployed: boolean }>>>();
  });
});

// ---------------------------------------------------------------------------
// Option namespace type safety
// ---------------------------------------------------------------------------

describe("Option namespace types", () => {
  it("Option.some(): T → Option<T>", () => {
    const action = O.some<string>();
    assertExact<IsExact<ExtractInput<typeof action>, string>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Option<string>>>();
  });

  it("Option.none(): void → Option<T>", () => {
    const action = O.none<number>();
    assertExact<IsExact<ExtractInput<typeof action>, void>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Option<number>>>();
  });

  it("Option.map(action): Option<T> → Option<U>", () => {
    const action = O.map<{ artifact: string }, { verified: boolean }>(verify);
    assertExact<IsExact<ExtractInput<typeof action>, Option<{ artifact: string }>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Option<{ verified: boolean }>>>();
  });

  it("Option.map composes in pipe", () => {
    const action = pipe(
      O.some<{ artifact: string }>(),
      O.map(verify),
    );
    assertExact<IsExact<ExtractInput<typeof action>, { artifact: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Option<{ verified: boolean }>>>();
  });

  it("Option.andThen(action): Option<T> → Option<U>", () => {
    // andThen chains into an action that itself returns Option
    const action = O.andThen<{ artifact: string }, { verified: boolean }>(
      pipe(verify, O.some<{ verified: boolean }>()),
    );
    assertExact<IsExact<ExtractInput<typeof action>, Option<{ artifact: string }>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Option<{ verified: boolean }>>>();
  });

  it("Option.andThen composes in pipe for chaining", () => {
    const action = pipe(
      O.some<{ artifact: string }>(),
      O.andThen(pipe(verify, O.some<{ verified: boolean }>())),
    );
    assertExact<IsExact<ExtractInput<typeof action>, { artifact: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Option<{ verified: boolean }>>>();
  });

  it("Option.unwrapOr(defaultAction): Option<T> → T", () => {
    const action = O.unwrapOr<string>(constant("fallback"));
    assertExact<IsExact<ExtractInput<typeof action>, Option<string>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, string>>();
  });

  it("Option.flatten(): Option<Option<T>> → Option<T>", () => {
    const action = O.flatten<string>();
    assertExact<IsExact<ExtractInput<typeof action>, Option<Option<string>>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Option<string>>>();
  });

  it("Option.filter(predicate): Option<T> → Option<T>", () => {
    // Predicate that keeps strings longer than 3 chars (returns Option<string>)
    const predicate = pipe(
      identity<string>(),
      O.some<string>(), // trivial: always keep
    );
    const action = O.filter<string>(predicate);
    assertExact<IsExact<ExtractInput<typeof action>, Option<string>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Option<string>>>();
  });

  it("Option.collect(): Option<T>[] → T[]", () => {
    const action = O.collect<string>();
    assertExact<IsExact<ExtractInput<typeof action>, Option<string>[]>>();
    assertExact<IsExact<ExtractOutput<typeof action>, string[]>>();
  });

  it("Option.isSome(): Option<T> → boolean", () => {
    const action = O.isSome<string>();
    assertExact<IsExact<ExtractInput<typeof action>, Option<string>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, boolean>>();
  });

  it("Option.isNone(): Option<T> → boolean", () => {
    const action = O.isNone<number>();
    assertExact<IsExact<ExtractInput<typeof action>, Option<number>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, boolean>>();
  });

  it("full Option pipeline: construct → map → unwrapOr", () => {
    const action = pipe(
      O.some<{ artifact: string }>(),
      O.map(verify),
      O.unwrapOr(constant({ verified: false })),
    );
    assertExact<IsExact<ExtractInput<typeof action>, { artifact: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { verified: boolean }>>();
  });

  it("forEach + Option.collect pipeline", () => {
    const action = pipe(
      forEach(O.map<{ artifact: string }, { verified: boolean }>(verify)),
      O.collect<{ verified: boolean }>(),
    );
    assertExact<IsExact<ExtractInput<typeof action>, Option<{ artifact: string }>[]>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { verified: boolean }[]>>();
  });
});

// ---------------------------------------------------------------------------
// bind / bindInput types
// ---------------------------------------------------------------------------

describe("bind types", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("VarRef output type matches binding output", () => {
    // computeName: Pipeable<{ project: string }, string>
    const computeName = pipe(setup, extractField<{ initialized: boolean; project: string }, "project">("project"));

    bind([computeName], ([name]) => {
      // name should be VarRef<string> = TypedAction<never, string>
      assertExact<IsExact<typeof name, VarRef<string>>>();
      assertExact<IsExact<ExtractInput<typeof name>, never>>();
      assertExact<IsExact<ExtractOutput<typeof name>, string>>();
      return drop();
    });
  });

  it("VarRef pipes into action expecting matching input", () => {
    // verify expects { artifact: string }
    bind([constant({ artifact: "test" })], ([artifact]) => {
      // artifact: VarRef<{ artifact: string }>
      // Piping into verify should compile
      return pipe(artifact, verify);
    });
  });

  it("VarRef rejects piping into action expecting wrong input", () => {
    bind([constant("a string")], ([s]) => {
      // s: VarRef<string>, verify expects { artifact: string }
      // @ts-expect-error — string is not { artifact: string }
      return pipe(s, verify);
    });
  });

  it("multiple bindings infer distinct VarRef types", () => {
    const stringAction = constant("hello");
    const numberAction = constant(42);

    bind([stringAction, numberAction], ([s, n]) => {
      assertExact<IsExact<ExtractOutput<typeof s>, string>>();
      assertExact<IsExact<ExtractOutput<typeof n>, number>>();
      return drop();
    });
  });

  it("bind output type matches body output type", () => {
    const action = bind([constant("x")], ([_s]) => verify);
    assertExact<IsExact<ExtractOutput<typeof action>, { verified: boolean }>>();
  });

  it("bind input type matches binding input type", () => {
    const action = bind([setup], ([_env]) => constant("done"));
    assertExact<IsExact<ExtractInput<typeof action>, { project: string }>>();
  });
});

describe("bindInput types", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("infers VarRef type from explicit type parameter", () => {
    bindInput<{ artifact: string }, { verified: boolean }>((input) => {
      assertExact<IsExact<typeof input, VarRef<{ artifact: string }>>>();
      assertExact<IsExact<ExtractOutput<typeof input>, { artifact: string }>>();
      return pipe(input, verify);
    });
  });

  it("output type matches body return type", () => {
    const action = bindInput<{ artifact: string }, { verified: boolean }>((input) =>
      pipe(input, verify),
    );
    assertExact<IsExact<ExtractOutput<typeof action>, { verified: boolean }>>();
  });

  it("input type matches TIn parameter", () => {
    const action = bindInput<{ project: string }, string>((_input) =>
      constant("done"),
    );
    assertExact<IsExact<ExtractInput<typeof action>, { project: string }>>();
  });

  it("body pipeline input is never (must use VarRef)", () => {
    // The body function signature requires Pipeable<never, TOut>
    // A pipeline starting from a VarRef has input never, so it works
    bindInput<string, string>((input) => {
      // input: VarRef<string> = TypedAction<never, string>
      assertExact<IsExact<ExtractInput<typeof input>, never>>();
      return input;
    });
  });
});

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

describe("Result types", () => {
  it("Result.ok() input is TValue, output is Result<TValue, TError>", () => {
    const action = R.ok<string, number>();
    assertExact<IsExact<ExtractInput<typeof action>, string>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Result<string, number>>>();
  });

  it("Result.err() input is TError, output is Result<TValue, TError>", () => {
    const action = R.err<string, number>();
    assertExact<IsExact<ExtractInput<typeof action>, number>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Result<string, number>>>();
  });

  it("Result.map transforms Ok type, preserves Err type", () => {
    const action = R.map<string, number, boolean>(
      constant(42) as TypedAction<string, number>,
    );
    assertExact<IsExact<ExtractInput<typeof action>, Result<string, boolean>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Result<number, boolean>>>();
  });

  it("Result.mapErr transforms Err type, preserves Ok type", () => {
    const action = R.mapErr<string, number, boolean>(
      constant(true) as TypedAction<number, boolean>,
    );
    assertExact<IsExact<ExtractInput<typeof action>, Result<string, number>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Result<string, boolean>>>();
  });

  it("Result.andThen input is Result, output is Result with new Ok type", () => {
    const action = R.andThen<string, number, boolean>(
      constant({ kind: "Ok" as const, value: 42 }) as TypedAction<string, Result<number, boolean>>,
    );
    assertExact<IsExact<ExtractInput<typeof action>, Result<string, boolean>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Result<number, boolean>>>();
  });

  it("Result.or input is Result, output has new Err type", () => {
    const action = R.or<string, number, boolean>(
      constant({ kind: "Ok" as const, value: "x" }) as TypedAction<number, Result<string, boolean>>,
    );
    assertExact<IsExact<ExtractInput<typeof action>, Result<string, number>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Result<string, boolean>>>();
  });

  it("Result.and replaces Ok type, preserves Err type", () => {
    const action = R.and<string, number, boolean>(
      constant({ kind: "Ok" as const, value: 42 }) as TypedAction<never, Result<number, boolean>>,
    );
    assertExact<IsExact<ExtractInput<typeof action>, Result<string, boolean>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Result<number, boolean>>>();
  });

  it("Result.unwrapOr extracts TValue from Result", () => {
    const action = R.unwrapOr<string, number>(
      constant("fallback") as TypedAction<number, string>,
    );
    assertExact<IsExact<ExtractInput<typeof action>, Result<string, number>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, string>>();
  });

  it("Result.flatten unwraps nested Result", () => {
    const action = R.flatten<string, number>();
    assertExact<IsExact<ExtractInput<typeof action>, Result<Result<string, number>, number>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Result<string, number>>>();
  });

  it("Result.toOption converts to Option<TValue>", () => {
    const action = R.toOption<string, number>();
    assertExact<IsExact<ExtractInput<typeof action>, Result<string, number>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Option<string>>>();
  });

  it("Result.toOptionErr converts to Option<TError>", () => {
    const action = R.toOptionErr<string, number>();
    assertExact<IsExact<ExtractInput<typeof action>, Result<string, number>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Option<number>>>();
  });

  it("Result.transpose swaps Result/Option nesting", () => {
    const action = R.transpose<string, number>();
    assertExact<IsExact<ExtractInput<typeof action>, Result<Option<string>, number>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Option<Result<string, number>>>>();
  });

  it("Result.isOk returns boolean", () => {
    const action = R.isOk<string, number>();
    assertExact<IsExact<ExtractInput<typeof action>, Result<string, number>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, boolean>>();
  });

  it("Result.isErr returns boolean", () => {
    const action = R.isErr<string, number>();
    assertExact<IsExact<ExtractInput<typeof action>, Result<string, number>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, boolean>>();
  });

  it("Result branches with Ok/Err cases", () => {
    const action = pipe(
      R.ok<string, number>(),
      R.map<string, number, number>(constant(42) as TypedAction<string, number>),
      R.unwrapOr<number, number>(identity<number>()),
    );
    assertExact<IsExact<ExtractInput<typeof action>, string>>();
    assertExact<IsExact<ExtractOutput<typeof action>, number>>();
  });
});
