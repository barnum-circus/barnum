import { describe, it, expect, beforeEach } from "vitest";
import {
  type ExtractInput,
  type ExtractOutput,
  type VarRef,
  pipe,
  bind,
  bindInput,
  resetEffectIdCounter,
} from "../src/ast.js";
import { constant, drop, getField, identity } from "../src/builtins/index.js";
import { runPipeline } from "../src/run.js";
import { setup, verify } from "./handlers.js";

// ---------------------------------------------------------------------------
// Type assertion helpers (compile-time only)
// ---------------------------------------------------------------------------

type IsExact<T, U> = [T] extends [U] ? ([U] extends [T] ? true : false) : false;

// eslint-disable-next-line @typescript-eslint/no-unused-vars
function assertExact<_T extends true>(): void {}

// ---------------------------------------------------------------------------
// Type tests
// ---------------------------------------------------------------------------

describe("bind type tests", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("VarRef output type matches binding output", () => {
    const computeName = pipe(
      setup,
      getField<{ initialized: boolean; project: string }, "project">("project"),
    );

    bind([computeName], ([name]) => {
      assertExact<IsExact<typeof name, VarRef<string>>>();
      assertExact<IsExact<ExtractInput<typeof name>, any>>();
      assertExact<IsExact<ExtractOutput<typeof name>, string>>();
      return drop;
    });
  });

  it("VarRef pipes into action expecting matching input", () => {
    bind([constant({ artifact: "test" })], ([artifact]) =>
      pipe(artifact, verify),
    );
  });

  it("VarRef rejects piping into action expecting wrong input", () => {
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

describe("bindInput type tests", () => {
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
    const action = bindInput<{ artifact: string }, { verified: boolean }>(
      (input) => pipe(input, verify),
    );
    assertExact<IsExact<ExtractOutput<typeof action>, { verified: boolean }>>();
  });

  it("input type matches TIn parameter", () => {
    const action = bindInput<{ project: string }, string>((_input) =>
      constant("done"),
    );
    assertExact<IsExact<ExtractInput<typeof action>, { project: string }>>();
  });

  it("body pipeline input is any (VarRef ignores pipeline input)", () => {
    bindInput<string, string>((input) => {
      assertExact<IsExact<ExtractInput<typeof input>, any>>();
      return input;
    });
  });
});

// ---------------------------------------------------------------------------
// AST structure tests
// ---------------------------------------------------------------------------

describe("bind AST structure", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("single binding produces Chain(All(..., Identity), ResumeHandle(...))", () => {
    const exprA = constant(42);
    const bodyAction = identity();
    const result = bind([exprA], ([_a]) => bodyAction);

    expect(result.kind).toBe("Chain");
    const outer = result as { kind: "Chain"; first: any; rest: any };

    // First: All with 2 actions (binding + Identity)
    expect(outer.first.kind).toBe("All");
    expect(outer.first.actions).toHaveLength(2);
    expect(outer.first.actions[0]).toEqual(exprA);
    expect(outer.first.actions[1].handler.builtin.kind).toBe("Identity");

    // Rest: ResumeHandle
    expect(outer.rest.kind).toBe("ResumeHandle");
  });

  it("two bindings produce nested Handles with distinct effectIds", () => {
    const exprA = constant("alice");
    const exprB = constant(99);
    const result = bind([exprA, exprB], ([_a, _b]) => identity());

    const outer = result as { kind: "Chain"; first: any; rest: any };

    // All with 3 actions (2 bindings + Identity)
    expect(outer.first.kind).toBe("All");
    expect(outer.first.actions).toHaveLength(3);

    // Outer ResumeHandle
    const handle0 = outer.rest;
    expect(handle0.kind).toBe("ResumeHandle");

    // Inner ResumeHandle
    const handle1 = handle0.body;
    expect(handle1.kind).toBe("ResumeHandle");

    // Distinct resume_handler_ids
    expect(handle0.resume_handler_id).not.toBe(handle1.resume_handler_id);
  });

  it("VarRef is a ResumePerform node with unique resume_handler_id", () => {
    let capturedVarRef: any;
    bind([constant("x")], ([a]) => {
      capturedVarRef = a;
      return identity();
    });

    expect(capturedVarRef.kind).toBe("ResumePerform");
    expect(typeof capturedVarRef.resume_handler_id).toBe("number");
  });

  it("resume_handler_ids are unique across separate bind calls", () => {
    bind([constant(1), constant(2)], ([_a, _b]) => identity());

    let ref1: any, ref2: any;
    bind([constant(3), constant(4)], ([a, b]) => {
      ref1 = a;
      ref2 = b;
      return identity();
    });

    expect(ref1.resume_handler_id).not.toBe(0);
    expect(ref1.resume_handler_id).not.toBe(1);
    expect(ref2.resume_handler_id).not.toBe(0);
    expect(ref2.resume_handler_id).not.toBe(1);
    expect(ref1.resume_handler_id).not.toBe(ref2.resume_handler_id);
  });
});

describe("bindInput AST structure", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("compiles to bind([identity], ([input]) => body)", () => {
    const bodyAction = constant("result");
    const result = bindInput<string, string>((_input) => bodyAction);

    const outer = result as { kind: "Chain"; first: any; rest: any };
    expect(outer.first.kind).toBe("All");
    expect(outer.first.actions).toHaveLength(2);
    expect(outer.first.actions[0].handler.builtin.kind).toBe("Identity");
    expect(outer.first.actions[1].handler.builtin.kind).toBe("Identity");

    expect(outer.rest.kind).toBe("ResumeHandle");
  });

  it("VarRef from bindInput is a ResumePerform node", () => {
    let capturedRef: any;
    bindInput<string, string>((input) => {
      capturedRef = input;
      return constant("result");
    });

    expect(capturedRef.kind).toBe("ResumePerform");
    expect(typeof capturedRef.resume_handler_id).toBe("number");
  });
});

// ---------------------------------------------------------------------------
// Execution tests
// ---------------------------------------------------------------------------

describe("bind execution", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("bind with single constant binding: body receives value", async () => {
    const result = await runPipeline(bind([constant(42)], ([n]) => n));
    expect(result).toBe(42);
  });

  it("bind with two bindings: body receives both values", async () => {
    const result = await runPipeline(
      bind([constant("hello"), constant(99)], ([_s, n]) => n),
    );
    expect(result).toBe(99);
  });

  it("bind: pipeline input is available in body", async () => {
    const result = await runPipeline(
      pipe(
        constant({ x: 10 }),
        bind([constant("bound")], ([_s]) => getField("x")),
      ),
    );
    expect(result).toBe(10);
  });
});

describe("bindInput execution", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("captured input is available as VarRef", async () => {
    const result = await runPipeline(
      pipe(
        constant(42),
        bindInput<number, number>((input) => input),
      ),
    );
    expect(result).toBe(42);
  });

  it("VarRef value pipes into subsequent action", async () => {
    const result = await runPipeline(
      pipe(
        constant({ artifact: "test.build" }),
        bindInput<{ artifact: string }, { artifact: string }>((input) => input),
      ),
    );
    expect(result).toEqual({ artifact: "test.build" });
  });
});
