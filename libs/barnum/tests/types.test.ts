/**
 * Pure type-level tests. Every test here is a compile-time assertion —
 * if it type-checks, the test passes. Runtime assertions are minimal.
 */
import { describe, it, expect, beforeEach } from "vitest";
import { z } from "zod";
import { createHandler, createHandlerWithConfig } from "../src/handler.js";
import {
  type TaggedUnion,
  type TypedAction,
  type ExtractInput,
  type ExtractOutput,
  type LoopResult,
  type Option,
  type OptionDef,
  type Result,
  type VarRef,
  typedAction,
  pipe,
  all,
  forEach,
  branch,
  loop,
  config,
  bind,
  bindInput,
  resetEffectIdCounter,
  tryCatch,
  race,
  sleep,
  withTimeout,
} from "../src/ast.js";
import { allocateRestartHandlerId } from "../src/effect-id.js";
import {
  constant,
  identity,
  drop,
  merge,
  flatten,
  getField,
  range,
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
  it("constant: any -> T", () => {
    const action = constant({ x: 1 });
    assertExact<IsExact<ExtractInput<typeof action>, any>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { x: number }>>();
    expect(action.kind).toBe("Invoke");
  });

  it("identity: T -> T", () => {
    const action = identity;
    assertExact<IsExact<ExtractInput<typeof action>, { x: number }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { x: number }>>();
    expect(action.kind).toBe("Invoke");
  });

  it("drop: any -> never", () => {
    assertExact<IsExact<ExtractInput<typeof drop>, any>>();
    assertExact<IsExact<ExtractOutput<typeof drop>, never>>();
    expect(drop.kind).toBe("Invoke");
  });

  it("range: any -> number[]", () => {
    const action = range(0, 10);
    assertExact<IsExact<ExtractInput<typeof action>, any>>();
    assertExact<IsExact<ExtractOutput<typeof action>, number[]>>();
    expect(action.kind).toBe("Invoke");
  });

  it("flatten: T[][] -> T[]", () => {
    const action = flatten<number>();
    assertExact<IsExact<ExtractInput<typeof action>, number[][]>>();
    assertExact<IsExact<ExtractOutput<typeof action>, number[]>>();
    expect(action.kind).toBe("Invoke");
  });

  it("getField: { key: V } -> V", () => {
    const action = getField<{ name: string; age: number }, "name">("name");
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
    const action = loop<{ stable: true }, { deployed: boolean }>((recur, done) =>
      healthCheck.branch({ Continue: recur, Break: done }),
    );
    assertExact<IsExact<ExtractInput<typeof action>, { deployed: boolean }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { stable: true }>>();
    expect(action.kind).toBe("Chain");
  });

  it("loop with branch/recur/drop: output is never when done unused", () => {
    const action = loop((recur) =>
      pipe(
        typeCheck,
        classifyErrors,
      ).branch({
        HasErrors: pipe(forEach(fix).drop(), recur),
        Clean: drop,
      }),
    );
    // Loop output: never (TBreak defaults to never, done not used)
    assertExact<
      IsExact<ExtractOutput<typeof action>, never>
    >();
    expect(action.kind).toBe("Chain");
  });

  it("full pipeline: constant → handlers → forEach → loop", () => {
    const action = pipe(
      constant({ project: "test" }),
      setup,
      listFiles,
      forEach(migrate),
    ).then(loop((recur) =>
      pipe(
        typeCheck,
        classifyErrors,
      ).branch({
        HasErrors: pipe(forEach(fix).drop(), recur),
        Clean: drop,
      }),
    ));
    assertExact<IsExact<ExtractInput<typeof action>, any>>();
    assertExact<
      IsExact<ExtractOutput<typeof action>, never>
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
      Clean: drop,
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

  it(".getField(): extracts field from output", () => {
    const action = pipe(
      constant({ name: "test", age: 42 }),
    ).getField("name");
    assertExact<IsExact<ExtractInput<typeof action>, any>>();
    assertExact<IsExact<ExtractOutput<typeof action>, string>>();
    expect(action.kind).toBe("Chain");
  });

  it(".getField() chains with .then()", () => {
    const action = pipe(
      constant({ result: { data: [1, 2, 3] } }),
    ).getField("result").getField("data");
    assertExact<IsExact<ExtractOutput<typeof action>, number[]>>();
    expect(action.kind).toBe("Chain");
  });

  it("postfix .branch() + .drop() compose in chain", () => {
    const action = pipe(
      typeCheck,
      classifyErrors,
    ).branch({
      HasErrors: forEach(fix),
      Clean: drop,
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
      Clean: drop,
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
    classifyErrors.branch({ HasErrors: drop });
  });

  it("rejects wrong handler type in postfix branch", () => {
    // @ts-expect-error — deploy expects { verified: boolean }, not HasErrors variant
    classifyErrors.branch({ HasErrors: deploy, Clean: drop });
  });

  it("accepts exhaustive postfix branch with bare drop", () => {
    classifyErrors.branch({
      HasErrors: drop,
      Clean: drop,
    });
  });

  it("rejects .branch() on non-discriminated output", () => {
    // @ts-expect-error — Out has no kind, .branch() should reject
    deploy.branch({ A: drop });
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
    pipe(classifyErrors, branch({ HasErrors: drop }));
  });

  it("accepts exhaustive branch", () => {
    const action = pipe(
      classifyErrors,
      branch({ HasErrors: drop, Clean: drop }),
    );
    expect(action.kind).toBe("Chain");
  });
});

// ---------------------------------------------------------------------------
// Config entry point
// ---------------------------------------------------------------------------

describe("config entry point", () => {
  it("accepts workflows starting with constant", () => {
    const cfg = config(
      pipe(constant({ artifact: "test" }), verify),
    );
    expect(cfg.workflow.kind).toBe("Chain");
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
    assertExact<IsExact<ExtractInput<typeof mapped>, any>>();
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

  it("Option.none(): never → Option<T>", () => {
    const action = O.none<number>();
    assertExact<IsExact<ExtractInput<typeof action>, never>>();
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
      identity,
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
    const computeName = pipe(setup, getField<{ initialized: boolean; project: string }, "project">("project"));

    bind([computeName], ([name]) => {
      // name should be VarRef<string> = TypedAction<never, string>
      assertExact<IsExact<typeof name, VarRef<string>>>();
      assertExact<IsExact<ExtractInput<typeof name>, never>>();
      assertExact<IsExact<ExtractOutput<typeof name>, string>>();
      return drop;
    });
  });

  it("VarRef pipes into action expecting matching input", () => {
    // verify expects { artifact: string }
    // artifact: VarRef<{ artifact: string }>
    // Piping into verify should compile
    bind([constant({ artifact: "test" })], ([artifact]) =>
      pipe(artifact, verify),
    );
  });

  it("VarRef rejects piping into action expecting wrong input", () => {
    // s: VarRef<string>, verify expects { artifact: string }
    bind([constant("a string")], ([s]) =>
      // @ts-expect-error — string is not { artifact: string }
      pipe(s, verify),
    );
  });

  it("multiple bindings infer distinct VarRef types", () => {
    const stringAction = constant("hello");
    const numberAction = constant(42);

    bind([stringAction, numberAction], ([s, n]) => {
      assertExact<IsExact<ExtractOutput<typeof s>, string>>();
      assertExact<IsExact<ExtractOutput<typeof n>, number>>();
      return drop;
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
      R.unwrapOr<number, number>(identity),
    );
    assertExact<IsExact<ExtractInput<typeof action>, string>>();
    assertExact<IsExact<ExtractOutput<typeof action>, number>>();
  });
});

// ---------------------------------------------------------------------------
// tryCatch types
// ---------------------------------------------------------------------------

describe("tryCatch types", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("tryCatch: input from body, output matches body and recovery", () => {
    const action = tryCatch(
      (_throwError) => pipe(setup, build),
      // Recovery receives TError, must produce same output as body.
      // TError is unconstrained here (throwError unused), so drop + constant.
      pipe(drop, constant({ artifact: "fallback" })),
    );
    assertExact<IsExact<ExtractInput<typeof action>, { project: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { artifact: string }>>();
  });

  it("throwError token is TypedAction<TError, never>", () => {
    tryCatch(
      (throwError) => {
        assertExact<IsExact<typeof throwError, TypedAction<string, never>>>();
        return identity;
      },
      identity,
    );
  });

  it("recovery input type matches throwError payload type", () => {
    const action = tryCatch(
      (_throwError: TypedAction<{ code: number; msg: string }, never>) =>
        pipe(drop, constant("ok")),
      // Recovery receives { code: number; msg: string }, extracts msg
      getField<{ code: number; msg: string }, "msg">("msg"),
    );
    assertExact<IsExact<ExtractOutput<typeof action>, string>>();
  });

  it("nested tryCatch: each throwError has independent TError", () => {
    tryCatch(
      (throwOuter) => {
        assertExact<
          IsExact<
            typeof throwOuter,
            TypedAction<{ initialized: boolean; project: string }, never>
          >
        >();
        return tryCatch(
          (throwInner) => {
            assertExact<
              IsExact<typeof throwInner, TypedAction<{ artifact: string }, never>>
            >();
            return pipe(drop, constant({ verified: true }));
          },
          verify,
        );
      },
      pipe(build, verify),
    );
  });

  it("tryCatch produces Chain(Tag(Continue), Handle(...)) AST", () => {
    const action = tryCatch(
      (_throwError) => pipe(drop, constant("ok")),
      identity,
    );
    // Outer node is Chain(Tag("Continue"), Handle(...)) — restart+Branch pattern.
    expect(action.kind).toBe("Chain");
  });
});

// ---------------------------------------------------------------------------
// Result.unwrapOr + .unwrapOr() postfix with throw tokens
// ---------------------------------------------------------------------------

describe("Result.unwrapOr with throw tokens", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("Result.unwrapOr accepts throw token with explicit types", () => {
    const throwToken = typedAction<string, never>({ kind: "RestartPerform", restart_handler_id: allocateRestartHandlerId() });
    const action = R.unwrapOr<string, string>(throwToken);
    assertExact<IsExact<ExtractInput<typeof action>, Result<string, string>>>();
    assertExact<IsExact<ExtractOutput<typeof action>, string>>();
  });

  it(".unwrapOr() infers types from this constraint", () => {
    const resultAction = identity as TypedAction<string, Result<string, number>>;
    const throwToken = typedAction<number, never>({ kind: "RestartPerform", restart_handler_id: allocateRestartHandlerId() });
    const action = resultAction.unwrapOr(throwToken);
    assertExact<IsExact<ExtractInput<typeof action>, string>>();
    assertExact<IsExact<ExtractOutput<typeof action>, string>>();
  });

  it(".unwrapOr() composes in tryCatch pipeline", () => {
    const handler = identity as TypedAction<
      { data: string },
      Result<{ data: string }, { code: number }>
    >;
    const action = tryCatch(
      (throwError) => handler.unwrapOr(throwError),
      pipe(drop, constant({ data: "fallback" })),
    );
    assertExact<IsExact<ExtractInput<typeof action>, { data: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { data: string }>>();
  });

  it(".unwrapOr() chains into further pipeline steps", () => {
    const handler = identity as TypedAction<
      { artifact: string },
      Result<{ verified: boolean }, string>
    >;
    const action = tryCatch(
      (throwError) => pipe(
        handler.unwrapOr(throwError),
        deploy,
      ),
      pipe(drop, constant({ deployed: false })),
    );
    assertExact<IsExact<ExtractInput<typeof action>, { artifact: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { deployed: boolean }>>();
  });

  it(".unwrapOr() produces Chain AST node", () => {
    const resultAction = identity as TypedAction<void, Result<string, string>>;
    const throwToken = typedAction<string, never>({ kind: "RestartPerform", restart_handler_id: allocateRestartHandlerId() });
    const action = resultAction.unwrapOr(throwToken);
    expect(action.kind).toBe("Chain");
  });

  it("rejects .unwrapOr() on non-Result output", () => {
    // deploy outputs { deployed: boolean } — not a Result
    // @ts-expect-error — unwrapOr requires Out to be Result<TValue, TError>
    deploy.unwrapOr(drop);
  });
});

// ---------------------------------------------------------------------------
// race types
// ---------------------------------------------------------------------------

describe("race types", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("race: all branches same input/output, result matches", () => {
    const action = race(verify, verify);
    assertExact<IsExact<ExtractInput<typeof action>, { artifact: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { verified: boolean }>>();
  });

  it("race produces Chain(Tag(Continue), Handle(...)) AST", () => {
    const action = race(verify, verify);
    // Outer node is Chain(Tag("Continue"), Handle(...)) — restart+Branch pattern.
    expect(action.kind).toBe("Chain");
  });

  it("sleep: any → never (like drop)", () => {
    const action = sleep(1000);
    assertExact<IsExact<ExtractInput<typeof action>, any>>();
    assertExact<IsExact<ExtractOutput<typeof action>, never>>();
  });

  it("sleep produces Invoke AST", () => {
    const action = sleep(1000);
    expect(action.kind).toBe("Invoke");
  });
});

// ---------------------------------------------------------------------------
// withTimeout types
// ---------------------------------------------------------------------------

describe("withTimeout types", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("withTimeout: preserves input, wraps output in Result<TOut, void>", () => {
    const action = withTimeout(constant(5000), verify);
    assertExact<IsExact<ExtractInput<typeof action>, { artifact: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Result<{ verified: boolean }, void>>>();
  });

  it("withTimeout produces Chain(Tag(Continue), Handle(...)) AST", () => {
    const action = withTimeout(constant(1000), verify);
    // Outer node is Chain(Tag("Continue"), Handle(...)) — restart+Branch pattern.
    expect(action.kind).toBe("Chain");
  });

  it("withTimeout with any-input body", () => {
    const action = withTimeout(constant(3000), constant("result"));
    assertExact<IsExact<ExtractInput<typeof action>, any>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Result<string, void>>>();
  });
});

// ---------------------------------------------------------------------------
// Loop type parameter constraints
// ---------------------------------------------------------------------------
//
// loop<TBreak, TIn> — both default to never.
//
// For "terminate" loops (type-check-fix pattern), use drop in the
// termination case instead of done. The loop body completes without
// Perform, and the Handle exits normally. No type params needed.
//
// done is only needed for stateful loops where TBreak carries data
// (e.g., healthCheck returning { stable: true }).
//
// TBreak is listed first so stateful loops can write loop<BreakType>
// when TIn=never.

describe("loop type parameter constraints", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  // -- Pattern 1: terminate loop (type-check-fix) ----------------------------

  it("loop with drop in Clean case: zero type params", () => {
    const action = loop((recur) =>
      pipe(typeCheck, classifyErrors).branch({
        HasErrors: pipe(forEach(fix).drop(), recur),
        Clean: drop,
      }),
    );
    assertExact<IsExact<ExtractInput<typeof action>, any>>();
    assertExact<IsExact<ExtractOutput<typeof action>, never>>();
  });

  // -- Pattern 2: retry loop (retry-on-error) --------------------------------

  it("loop with done in unwrapOr: zero type params", () => {
    const stepC = R.ok<string, string>() as TypedAction<string, Result<string, string>>;

    loop((_recur, done) => {
      // unwrapOr(done) works because done's input is never,
      // and mapErr(drop) erases the error type to never — exact match.
      const unwrapped = stepC.mapErr(drop).unwrapOr(done);
      assertExact<IsExact<ExtractOutput<typeof unwrapped>, string>>();
      return done;
    });
  });

  // -- Pattern 3: stateful loop (healthCheck style) --------------------------

  it("loop<TBreak, TIn>: both explicit for stateful loops", () => {
    const action = loop<{ stable: true }, { deployed: boolean }>((recur, done) =>
      healthCheck.branch({ Continue: recur, Break: done }),
    );
    assertExact<IsExact<ExtractInput<typeof action>, { deployed: boolean }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { stable: true }>>();
  });

  // -- done defaults to never, can't accept non-never values -----------------

  it("without explicit TBreak, done has input=never (rejects void)", () => {
    loop((_recur, done) => {
      assertExact<IsExact<ExtractInput<typeof done>, never>>();
      // @ts-expect-error — done: TypedAction<never, never> can't accept void from Clean
      classifyErrors.branch({ HasErrors: forEach(fix), Clean: done });
      return done;
    });
  });

  it("without explicit TBreak, done has input=never (rejects objects)", () => {
    loop((_recur, done) => {
      // @ts-expect-error — done: TypedAction<never, never> can't accept { deployed: boolean }
      healthCheck.branch({ Continue: drop, Break: done });
      return done;
    });
  });

  // -- Token types -----------------------------------------------------------

  it("done and recur both output never (they're terminal)", () => {
    loop((recur, done) => {
      assertExact<IsExact<ExtractOutput<typeof recur>, never>>();
      assertExact<IsExact<ExtractOutput<typeof done>, never>>();
      return done;
    });
  });

  it("recur's input type is TIn", () => {
    loop<{ stable: true }, { deployed: boolean }>((recur, _done) => {
      assertExact<IsExact<ExtractInput<typeof recur>, { deployed: boolean }>>();
      return healthCheck.branch({ Continue: recur, Break: _done });
    });
  });

  it("done's input type is TBreak", () => {
    loop<{ stable: true }, { deployed: boolean }>((_recur, done) => {
      assertExact<IsExact<ExtractInput<typeof done>, { stable: true }>>();
      return healthCheck.branch({ Continue: _recur, Break: done });
    });
  });

  // -- PipeIn ----------------------------------------------------------------

  it("loop with TIn=never has PipeIn input (accepts any)", () => {
    const action = loop((recur) =>
      pipe(typeCheck, classifyErrors).branch({
        HasErrors: pipe(forEach(fix).drop(), recur),
        Clean: drop,
      }),
    );
    assertExact<IsExact<ExtractInput<typeof action>, any>>();
  });

  it("loop with explicit TIn has exact input", () => {
    const action = loop<{ stable: true }, { deployed: boolean }>((recur, done) =>
      healthCheck.branch({ Continue: recur, Break: done }),
    );
    assertExact<IsExact<ExtractInput<typeof action>, { deployed: boolean }>>();
  });

  // -- .drop() before recur in mid-pipe positions ----------------------------

  it(".drop() is required before recur in mid-pipe positions", () => {
    loop((recur) =>
      pipe(typeCheck, classifyErrors).branch({
        HasErrors: pipe(forEach(fix).drop(), recur),
        Clean: drop,
      }),
    );
  });
});

// ---------------------------------------------------------------------------
// Optional handler types — createHandler
// ---------------------------------------------------------------------------

describe("optional handler types: createHandler", () => {
  // --- inputValidator infers TValue (existing behavior, preserved) ---

  it("inputValidator infers TValue", () => {
    const h = createHandler({
      inputValidator: z.object({ name: z.string() }),
      handle: async ({ value }) => value.name.length,
    }, "h");
    assertExact<IsExact<ExtractInput<typeof h>, { name: string }>>();
    assertExact<IsExact<ExtractOutput<typeof h>, number>>();
  });

  it("inputValidator + outputValidator infers both", () => {
    const h = createHandler({
      inputValidator: z.string(),
      outputValidator: z.number(),
      handle: async ({ value }) => value.length,
    }, "h");
    assertExact<IsExact<ExtractInput<typeof h>, string>>();
    assertExact<IsExact<ExtractOutput<typeof h>, number>>();
  });

  // --- source handler (no inputValidator) ---

  it("source handler: input is never", () => {
    const h = createHandler({
      handle: async () => "hello",
    }, "h");
    assertExact<IsExact<ExtractInput<typeof h>, never>>();
    assertExact<IsExact<ExtractOutput<typeof h>, string>>();
  });

  it("source handler with outputValidator", () => {
    const h = createHandler({
      outputValidator: z.array(z.string()),
      handle: async () => ["a", "b"],
    }, "h");
    assertExact<IsExact<ExtractInput<typeof h>, never>>();
    assertExact<IsExact<ExtractOutput<typeof h>, string[]>>();
  });

  // --- explicit type params without validators ---

  it("explicit type params: typed input without validator", () => {
    const h = createHandler<{ id: number }, string>({
      handle: async ({ value }) => String(value.id),
    }, "h");
    assertExact<IsExact<ExtractInput<typeof h>, { id: number }>>();
    assertExact<IsExact<ExtractOutput<typeof h>, string>>();
  });

  it("explicit type params with outputValidator", () => {
    const h = createHandler<string, number>({
      outputValidator: z.number(),
      handle: async ({ value }) => value.length,
    }, "h");
    assertExact<IsExact<ExtractInput<typeof h>, string>>();
    assertExact<IsExact<ExtractOutput<typeof h>, number>>();
  });

  // --- handle must match declared types ---

  it("rejects handle that returns wrong type for explicit TOutput", () => {
    // @ts-expect-error — handle returns string, TOutput is number
    createHandler<string, number>({ handle: async ({ value }) => value.toUpperCase() }, "h");
  });

  it("rejects handle that uses wrong type for explicit TValue", () => {
    createHandler<string, number>({
      // @ts-expect-error — value is string, not number; .toFixed doesn't exist
      handle: async ({ value }) => value.toFixed(2),
    }, "h");
  });

  // --- validators must match declared types ---

  it("rejects inputValidator that contradicts explicit TValue", () => {
    // @ts-expect-error — TValue is string but validator is z.number()
    createHandler<string, number>({ inputValidator: z.number(), handle: async ({ value }) => value.length }, "h");
  });

  it("rejects outputValidator that contradicts explicit TOutput", () => {
    // @ts-expect-error — TOutput is number but validator is z.string()
    createHandler<string, number>({ inputValidator: z.string(), outputValidator: z.string(), handle: async ({ value }) => value.length }, "h");
  });

  it("rejects outputValidator that contradicts inferred TOutput", () => {
    // @ts-expect-error — handle returns number, outputValidator is z.string()
    createHandler({ inputValidator: z.string(), outputValidator: z.string(), handle: async ({ value }) => value.length }, "h");
  });

  it("rejects inputValidator that contradicts handle parameter", () => {
    // @ts-expect-error — validator says number, handle destructures string methods
    createHandler({ inputValidator: z.number(), handle: async ({ value }) => value.toUpperCase() }, "h");
  });

  // --- validators must match explicit types (wider rejects, narrower allowed by covariance) ---

  it("rejects inputValidator wider than explicit TValue", () => {
    // @ts-expect-error — TValue is "hello" but validator accepts any string
    createHandler<"hello", string>({ inputValidator: z.string(), handle: async ({ value }) => value }, "h");
  });

  it("rejects outputValidator wider than explicit TOutput", () => {
    // @ts-expect-error — TOutput is "ok" but validator accepts any string
    createHandler<string, "ok">({ inputValidator: z.string(), outputValidator: z.string(), handle: async ({ value }) => "ok" as const }, "h");
  });

  it("accepts inputValidator that exactly matches explicit TValue", () => {
    const h = createHandler<string, number>({
      inputValidator: z.string(),
      handle: async ({ value }) => value.length,
    }, "h");
    assertExact<IsExact<ExtractInput<typeof h>, string>>();
    assertExact<IsExact<ExtractOutput<typeof h>, number>>();
  });

  it("accepts outputValidator that exactly matches explicit TOutput", () => {
    const h = createHandler<string, number>({
      inputValidator: z.string(),
      outputValidator: z.number(),
      handle: async ({ value }) => value.length,
    }, "h");
    assertExact<IsExact<ExtractInput<typeof h>, string>>();
    assertExact<IsExact<ExtractOutput<typeof h>, number>>();
  });

  // --- source handlers in workflows ---

  it("source handler is accepted as config entry point", () => {
    const h = createHandler({
      handle: async () => "result",
    }, "h");
    config(h);
  });

  // --- pipeline composition ---

  it("validator-typed handlers compose in pipe", () => {
    const toLength = createHandler({
      inputValidator: z.string(),
      handle: async ({ value }) => value.length,
    }, "toLength");
    const double = createHandler({
      inputValidator: z.number(),
      handle: async ({ value }) => value * 2,
    }, "double");
    const p = pipe(toLength, double);
    assertExact<IsExact<ExtractInput<typeof p>, string>>();
    assertExact<IsExact<ExtractOutput<typeof p>, number>>();
  });

  it("explicit-typed handlers compose in pipe", () => {
    const toLength = createHandler<string, number>({
      handle: async ({ value }) => value.length,
    }, "toLength");
    const double = createHandler<number, number>({
      handle: async ({ value }) => value * 2,
    }, "double");
    const p = pipe(toLength, double);
    assertExact<IsExact<ExtractInput<typeof p>, string>>();
    assertExact<IsExact<ExtractOutput<typeof p>, number>>();
  });

  it("mixed validator + explicit compose in pipe", () => {
    const toLength = createHandler({
      inputValidator: z.string(),
      handle: async ({ value }) => value.length,
    }, "toLength");
    const double = createHandler<number, number>({
      handle: async ({ value }) => value * 2,
    }, "double");
    const p = pipe(toLength, double);
    assertExact<IsExact<ExtractInput<typeof p>, string>>();
    assertExact<IsExact<ExtractOutput<typeof p>, number>>();
  });

  it("pipe rejects mismatched adjacent types", () => {
    const toString = createHandler({
      inputValidator: z.string(),
      handle: async ({ value }) => value.toUpperCase(),
    }, "toString");
    const fromNumber = createHandler({
      inputValidator: z.number(),
      handle: async ({ value }) => value * 2,
    }, "fromNumber");
    // @ts-expect-error — toString outputs string, fromNumber expects number
    pipe(toString, fromNumber);
  });

  it("source handler composes at pipe start", () => {
    const source = createHandler({
      handle: async () => 42,
    }, "source");
    const double = createHandler({
      inputValidator: z.number(),
      handle: async ({ value }) => value * 2,
    }, "double");
    const p = pipe(source, double);
    assertExact<IsExact<ExtractInput<typeof p>, any>>();
    assertExact<IsExact<ExtractOutput<typeof p>, number>>();
  });

  it("postfix .then() works with explicit-typed handler", () => {
    const toLength = createHandler({
      inputValidator: z.string(),
      handle: async ({ value }) => value.length,
    }, "toLength");
    const double = createHandler<number, number>({
      handle: async ({ value }) => value * 2,
    }, "double");
    const chained = toLength.then(double);
    assertExact<IsExact<ExtractInput<typeof chained>, string>>();
    assertExact<IsExact<ExtractOutput<typeof chained>, number>>();
  });
});

// ---------------------------------------------------------------------------
// Optional handler types — createHandlerWithConfig
// ---------------------------------------------------------------------------

describe("optional handler types: createHandlerWithConfig", () => {
  // --- stepConfigValidator optional ---

  it("omitting stepConfigValidator: stepConfig is unknown", () => {
    const factory = createHandlerWithConfig({
      handle: async ({ stepConfig }) => String(stepConfig),
    }, "h");
    const action = factory("anything");
    assertExact<IsExact<ExtractInput<typeof action>, never>>();
    assertExact<IsExact<ExtractOutput<typeof action>, string>>();
  });

  it("stepConfigValidator provided: stepConfig is typed", () => {
    const factory = createHandlerWithConfig({
      stepConfigValidator: z.object({ retries: z.number() }),
      handle: async ({ stepConfig }) => stepConfig.retries,
    }, "h");
    const action = factory({ retries: 3 });
    assertExact<IsExact<ExtractInput<typeof action>, never>>();
    assertExact<IsExact<ExtractOutput<typeof action>, number>>();
  });

  it("explicit TStepConfig without validator", () => {
    const factory = createHandlerWithConfig<never, string, { retries: number }>({
      handle: async ({ stepConfig }) => String(stepConfig.retries),
    }, "h");
    const action = factory({ retries: 3 });
    assertExact<IsExact<ExtractInput<typeof action>, never>>();
    assertExact<IsExact<ExtractOutput<typeof action>, string>>();
  });

  // --- inputValidator optional ---

  it("with inputValidator: input is typed", () => {
    const factory = createHandlerWithConfig({
      inputValidator: z.string(),
      stepConfigValidator: z.object({ retries: z.number() }),
      handle: async ({ value, stepConfig }) => `${value}:${stepConfig.retries}`,
    }, "h");
    const action = factory({ retries: 3 });
    assertExact<IsExact<ExtractInput<typeof action>, string>>();
    assertExact<IsExact<ExtractOutput<typeof action>, string>>();
  });

  it("without inputValidator: input defaults to never", () => {
    const factory = createHandlerWithConfig({
      handle: async ({ value, stepConfig: _stepConfig }) => String(value),
    }, "h");
    const action = factory("anything");
    assertExact<IsExact<ExtractInput<typeof action>, never>>();
    assertExact<IsExact<ExtractOutput<typeof action>, string>>();
  });

  it("explicit type params without inputValidator", () => {
    const factory = createHandlerWithConfig<string, number, { retries: number }>({
      handle: async ({ value, stepConfig }) => value.length + stepConfig.retries,
    }, "h");
    const action = factory({ retries: 3 });
    assertExact<IsExact<ExtractInput<typeof action>, string>>();
    assertExact<IsExact<ExtractOutput<typeof action>, number>>();
  });

  // --- all validators present ---

  it("all three validators", () => {
    const factory = createHandlerWithConfig({
      inputValidator: z.string(),
      outputValidator: z.number(),
      stepConfigValidator: z.object({ retries: z.number() }),
      handle: async ({ value, stepConfig }) => value.length + stepConfig.retries,
    }, "h");
    const action = factory({ retries: 3 });
    assertExact<IsExact<ExtractInput<typeof action>, string>>();
    assertExact<IsExact<ExtractOutput<typeof action>, number>>();
  });

  // --- type errors when lying ---

  it("rejects wrong stepConfigValidator", () => {
    // @ts-expect-error — explicit TStepConfig is { retries: number }, validator is z.string()
    createHandlerWithConfig<never, string, { retries: number }>({ stepConfigValidator: z.string(), handle: async ({ stepConfig }) => String(stepConfig) }, "h");
  });

  it("rejects handle that lies about stepConfig shape", () => {
    createHandlerWithConfig({
      stepConfigValidator: z.object({ retries: z.number() }),
      // @ts-expect-error — stepConfig.retries is number, not string method
      handle: async ({ stepConfig }) => stepConfig.retries.toUpperCase(),
    }, "h");
  });

  it("rejects outputValidator contradicting handle return", () => {
    // @ts-expect-error — handle returns number, outputValidator is z.string()
    createHandlerWithConfig({ outputValidator: z.string(), handle: async ({ stepConfig }) => 42 }, "h");
  });

  // --- validators must match explicit types (wider rejects) ---

  it("rejects stepConfigValidator wider than explicit TStepConfig", () => {
    // @ts-expect-error — TStepConfig is { retries: 3 } but validator accepts any { retries: number }
    createHandlerWithConfig<never, string, { retries: 3 }>({ stepConfigValidator: z.object({ retries: z.number() }), handle: async ({ stepConfig }) => String(stepConfig.retries) }, "h");
  });

  it("accepts all validators exactly matching explicit types", () => {
    const factory = createHandlerWithConfig<string, number, { retries: number }>({
      inputValidator: z.string(),
      outputValidator: z.number(),
      stepConfigValidator: z.object({ retries: z.number() }),
      handle: async ({ value, stepConfig }) => value.length + stepConfig.retries,
    }, "h");
    const action = factory({ retries: 3 });
    assertExact<IsExact<ExtractInput<typeof action>, string>>();
    assertExact<IsExact<ExtractOutput<typeof action>, number>>();
  });

  // --- pipeline composition ---

  it("withConfig handler composes in pipe", () => {
    const source = createHandler({
      handle: async () => "hello",
    }, "source");
    const withRetries = createHandlerWithConfig({
      inputValidator: z.string(),
      stepConfigValidator: z.object({ retries: z.number() }),
      handle: async ({ value, stepConfig }) => `${value}:${stepConfig.retries}`,
    }, "withRetries");
    const p = pipe(source, withRetries({ retries: 3 }));
    assertExact<IsExact<ExtractInput<typeof p>, any>>();
    assertExact<IsExact<ExtractOutput<typeof p>, string>>();
  });

  it("explicit-typed withConfig handler composes in pipe", () => {
    const source = createHandler({
      handle: async () => "hello",
    }, "source");
    const transform = createHandlerWithConfig<string, number, { n: number }>({
      handle: async ({ value, stepConfig }) => value.length + stepConfig.n,
    }, "transform");
    const p = pipe(source, transform({ n: 10 }));
    assertExact<IsExact<ExtractInput<typeof p>, any>>();
    assertExact<IsExact<ExtractOutput<typeof p>, number>>();
  });
});

