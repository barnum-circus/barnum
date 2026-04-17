import { describe, it, expect } from "vitest";
import {
  type ExtractInput,
  type ExtractOutput,
  type TypedAction,
  pipe,
  branch,
  forEach,
} from "../src/ast.js";
import {
  constant,
  drop,
  getField,
  identity,
  tag,
  wrapInField,
} from "../src/builtins/index.js";
import { runPipeline } from "../src/run.js";
import {
  deploy,
  verify,
  classifyErrors,
  fix,
  type TypeError,
  type ClassifyResult,
} from "./handlers.js";

// ---------------------------------------------------------------------------
// Type assertion helpers (compile-time only)
// ---------------------------------------------------------------------------

type IsExact<T, U> = [T] extends [U] ? ([U] extends [T] ? true : false) : false;

// eslint-disable-next-line @typescript-eslint/no-unused-vars
function assertExact<_T extends true>(): void {}

// ---------------------------------------------------------------------------
// Type tests
// ---------------------------------------------------------------------------

describe("branch type tests", () => {
  it("branch: input is discriminated union with kind, output is case union", () => {
    const action = branch({
      Yes: deploy,
      No: deploy,
    });
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

  it("postfix .branch(): input preserved, output is union of case outputs", () => {
    const action = classifyErrors.branch({
      HasErrors: forEach(fix),
      Clean: drop,
    });
    assertExact<IsExact<ExtractInput<typeof action>, TypeError[]>>();
    assertExact<
      IsExact<
        ExtractOutput<typeof action>,
        { file: string; fixed: boolean }[] | void
      >
    >();
    expect(action.kind).toBe("Chain");
  });

  it("postfix .branch() + .drop() compose in chain", () => {
    const action = pipe(
      constant(null) as TypedAction<any, TypeError[]>,
      classifyErrors,
    ).branch({
      HasErrors: forEach(fix),
      Clean: drop,
    });
    expect(action.kind).toBe("Chain");
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
// { kind, value } convention
// ---------------------------------------------------------------------------

describe("{ kind, value } convention", () => {
  it("ClassifyResult uses { kind, value } form", () => {
    assertExact<IsExact<
      Extract<ClassifyResult, { kind: "ClassifyResult.HasErrors" }>,
      { kind: "ClassifyResult.HasErrors"; value: TypeError[] }
    >>();
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
    assertExact<IsExact<
      Extract<ClassifyResult, { kind: "ClassifyResult.HasErrors" }>,
      { kind: "ClassifyResult.HasErrors"; value: TypeError[]; __def?: Def }
    >>();
  });
});

// ---------------------------------------------------------------------------
// Postfix .map() type safety
// ---------------------------------------------------------------------------

describe("postfix .map() type safety", () => {
  it("rejects .map() when Out is not Option<T> or Result<T,E>", () => {
    // verify outputs { verified: boolean } — not an Option or Result
    // @ts-expect-error — map requires Option or Result output
    verify.map(deploy);
  });

  it("rejects .map() when Out is a different tagged union", () => {
    // classifyErrors outputs ClassifyResult — not Option or Result
    // @ts-expect-error — map requires Option or Result output
    classifyErrors.map(deploy);
  });
});

// ---------------------------------------------------------------------------
// AST structure tests
// ---------------------------------------------------------------------------

describe("branch AST structure", () => {
  it("branch accepts cases with the same output type", () => {
    const workflow = branch({
      Yes: deploy,
      No: deploy,
    });
    expect(workflow.kind).toBe("Branch");
  });

  it(".branch() produces Chain → Branch AST", () => {
    const action = classifyErrors.branch({
      HasErrors: drop,
      Clean: drop,
    });
    expect(action.kind).toBe("Chain");
    const chain = action as { kind: "Chain"; first: any; rest: any };
    expect(chain.first.kind).toBe("Invoke");
    expect(chain.rest.kind).toBe("Branch");
    expect(Object.keys(chain.rest.cases)).toEqual(["HasErrors", "Clean"]);
  });

  it("postfix .branch() produces valid AST for loop pattern", () => {
    const action = pipe(
      constant(null) as TypedAction<any, TypeError[]>,
      classifyErrors,
    ).branch({
      HasErrors: forEach(fix),
      Clean: drop,
    });
    expect(action.kind).toBe("Chain");
    const chain = action as { kind: "Chain"; first: any; rest: any };
    expect(chain.rest.kind).toBe("Branch");
    expect(Object.keys(chain.rest.cases)).toEqual(["HasErrors", "Clean"]);
  });

  it("rejects output flowing into incompatible step", () => {
    // @ts-expect-error — branch output ({ deployed: boolean }) doesn't match verify's input ({ artifact: string })
    pipe(branch({ A: deploy, B: deploy }), verify);
  });
});

// ---------------------------------------------------------------------------
// Execution tests
// ---------------------------------------------------------------------------

describe("branch execution", () => {
  it("branch dispatches on kind string, extracts value, routes to correct case", async () => {
    const action = pipe(
      constant({ kind: "Left", value: 42 } as { kind: "Left"; value: number } | { kind: "Right"; value: string }),
      branch({
        Left: wrapInField("num"),
        Right: wrapInField("str"),
      }),
    );
    const result = await runPipeline(action);
    expect(result).toEqual({ num: 42 });
  });

  it("branch selects the other case", async () => {
    const action = pipe(
      constant({ kind: "Right", value: "hello" } as { kind: "Left"; value: number } | { kind: "Right"; value: string }),
      branch({
        Left: wrapInField("num"),
        Right: wrapInField("str"),
      }),
    );
    const result = await runPipeline(action);
    expect(result).toEqual({ str: "hello" });
  });

  it("branch with 3+ cases selects the right one", async () => {
    const action = pipe(
      constant({ kind: "Green", value: 128 } as
        | { kind: "Red"; value: number }
        | { kind: "Green"; value: number }
        | { kind: "Blue"; value: number }),
      branch({
        Red: constant("red"),
        Green: constant("green"),
        Blue: constant("blue"),
      }),
    );
    const result = await runPipeline(action);
    expect(result).toBe("green");
  });

  it("postfix .branch() dispatches correctly on tagged union", async () => {
    type ABDef = { A: number; B: string };
    const action = pipe(
      constant(10),
      tag<"AB", ABDef, "A">("A", "AB"),
    ).branch({
      A: identity(),
      B: drop,
    });
    const result = await runPipeline(action);
    expect(result).toBe(10);
  });

  it("branch extracts value before passing to handler", async () => {
    const action = pipe(
      constant({ kind: "Item", value: { x: 99 } } as { kind: "Item"; value: { x: number } }),
      branch({
        Item: getField("x"),
      }),
    );
    const result = await runPipeline(action);
    expect(result).toBe(99);
  });
});
