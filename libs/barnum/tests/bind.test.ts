import { beforeEach, describe, expect, it } from "vitest";
import {
  type VarRef,
  all,
  bind,
  bindInput,
  pipe,
  resetEffectIdCounter,
} from "../src/ast.js";
import { constant, drop, getField, identity } from "../src/builtins/index.js";
import { setup, verify } from "./handlers.js";
import { type IsExact, assertExact, assertIO } from "./type-utils.js";

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
      assertIO<typeof name, any, string>();
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
      assertIO<typeof s, any, string>();
      assertIO<typeof n, any, number>();
      return drop;
    });
  });

  it("bind output type matches body output type", () => {
    const action = bind([constant("x")], ([_s]) => verify);
    assertIO<typeof action, any, { verified: boolean }>();
  });

  it("bind input type matches binding input type", () => {
    const action = bind([setup], ([_env]) => constant("done"));
    assertIO<typeof action, { project: string }, string>();
  });
});

describe("bindInput type tests", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("infers VarRef type from explicit type parameter", () => {
    bindInput<{ artifact: string }, { verified: boolean }>((input) => {
      assertExact<IsExact<typeof input, VarRef<{ artifact: string }>>>();
      assertIO<typeof input, any, { artifact: string }>();
      return pipe(input, verify);
    });
  });

  it("output type matches body return type", () => {
    const action = bindInput<{ artifact: string }, { verified: boolean }>(
      (input) => pipe(input, verify),
    );
    assertIO<typeof action, { artifact: string }, { verified: boolean }>();
  });

  it("input type matches TIn parameter", () => {
    const action = bindInput<{ project: string }, string>((_input) =>
      constant("done"),
    );
    assertIO<typeof action, { project: string }, string>();
  });

  it("body pipeline input is any (VarRef ignores pipeline input)", () => {
    bindInput<string, string>((input) => {
      assertIO<typeof input, any, string>();
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
    const result = await bind([constant(42)], ([n]) => n).run();
    expect(result).toBe(42);
  });

  it("bind with two bindings: body receives both values", async () => {
    const result = await bind(
      [constant("hello"), constant(99)],
      ([_s, n]) => n,
    ).run();
    expect(result).toBe(99);
  });

  it("bind: pipeline input is available in body", async () => {
    const result = await pipe(
      constant({ x: 10 }),
      bind([constant("bound")], ([_s]) => getField("x")),
    ).run();
    expect(result).toBe(10);
  });
});

describe("bindInput execution", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("captured input is available as VarRef", async () => {
    const result = await pipe(
      constant(42),
      bindInput<number, number>((input) => input),
    ).run();
    expect(result).toBe(42);
  });

  it("VarRef value pipes into subsequent action", async () => {
    const result = await pipe(
      constant({ artifact: "test.build" }),
      bindInput<{ artifact: string }, { artifact: string }>((input) => input),
    ).run();
    expect(result).toEqual({ artifact: "test.build" });
  });
});

// ---------------------------------------------------------------------------
// .split() execution tests
// ---------------------------------------------------------------------------

describe("split execution", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("tuple split: both components accessible", async () => {
    const result = await pipe(
      constant(["hello", 42] as [string, number]),
      bindInput<[string, number], string>((state) => {
        const [str, _num] = state.split();
        return str;
      }),
    ).run();
    expect(result).toBe("hello");
  });

  it("tuple split: second component accessible", async () => {
    const result = await pipe(
      constant(["hello", 42] as [string, number]),
      bindInput<[string, number], number>((state) => {
        const [_str, num] = state.split();
        return num;
      }),
    ).run();
    expect(result).toBe(42);
  });

  it("object split: field accessible", async () => {
    const result = await pipe(
      constant({ name: "alice", age: 30 }),
      bindInput<{ name: string; age: number }, string>((state) => {
        const { name } = state.split();
        return name;
      }),
    ).run();
    expect(result).toBe("alice");
  });

  it("multiple splits of same VarRef yield independent results", async () => {
    const result = await pipe(
      constant({ a: { b: "nested" }, c: 99 }),
      bindInput<{ a: { b: string }; c: number }, string>((vr) => {
        const { a } = vr.split();
        const { c: _c } = vr.split();
        return a.getField("b");
      }),
    ).run();
    expect(result).toBe("nested");
  });

  it("split same VarRef for different nested fields", async () => {
    const result = await pipe(
      constant({ a: { b: "deep" }, x: 42 }),
      bindInput<{ a: { b: string }; x: number }, [string, number]>((vr) => {
        const { a } = vr.split();
        const { x } = vr.split();
        return all(a.getField("b"), x);
      }),
    ).run();
    expect(result).toEqual(["deep", 42]);
  });

  it("split VarRef then split nested field", async () => {
    const result = await pipe(
      constant({ outer: { inner: "found" } }),
      bindInput<{ outer: { inner: string } }, string>((vr) => {
        const { outer } = vr.split();
        const { inner } = outer.split();
        return inner;
      }),
    ).run();
    expect(result).toBe("found");
  });
});
