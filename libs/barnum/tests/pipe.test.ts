import { describe, expect, it } from "vitest";
import { branch, config, pipe, tap } from "../src/ast.js";
import { constant, drop, getField, identity } from "../src/builtins/index.js";
import { chain } from "../src/chain.js";
import { build, classifyErrors, deploy, setup, verify } from "./handlers.js";
import { assertIO } from "./type-utils.js";

// ---------------------------------------------------------------------------
// Type tests
// ---------------------------------------------------------------------------

describe("pipe type tests", () => {
  it("pipe: input of first, output of last", () => {
    const action = pipe(setup, build, verify);
    assertIO<typeof action, { project: string }, { verified: boolean }>();
    expect(action.kind).toBe("Chain");
  });

  it("rejects mismatched adjacent types", () => {
    // verify outputs { verified: boolean }, setup expects { project: string }
    // @ts-expect-error — output/input mismatch
    pipe(verify, setup);
  });

  it("rejects unrelated types", () => {
    // deploy outputs { deployed: boolean }, setup expects { project: string }
    // @ts-expect-error — output/input mismatch
    pipe(deploy, setup);
  });

  it("accepts compatible types", () => {
    const action = pipe(setup, build);
    expect(action.kind).toBe("Chain");
  });

  it("rejects non-exhaustive branch (missing case)", () => {
    // @ts-expect-error — non-exhaustive: missing "Clean" case
    pipe(classifyErrors, branch({ HasErrors: drop }));
  });

  it("accepts exhaustive branch", () => {
    const action = classifyErrors.branch({ HasErrors: drop, Clean: drop });
    expect(action.kind).toBe("Chain");
  });

  it("config accepts workflows starting with constant", () => {
    const cfg = config(pipe(constant({ artifact: "test" }), verify));
    expect(cfg.workflow.kind).toBe("Chain");
  });

  it("full pipeline: constant → handlers → forEach → branch", () => {
    const action = pipe(
      constant({ project: "test" }),
      setup,
      build,
      verify,
      deploy,
    );
    assertIO<typeof action, any, { deployed: boolean }>();
    expect(action.kind).toBe("Chain");
  });
});

// ---------------------------------------------------------------------------
// AST structure tests
// ---------------------------------------------------------------------------

describe("pipe AST structure", () => {
  it("pipe chains setup → build → verify → deploy", () => {
    const cfg = config(
      pipe(constant({ project: "test" }), setup, build, verify, deploy),
    );
    expect(cfg.workflow.kind).toBe("Chain");
  });

  it("pipe chains three steps correctly", () => {
    const workflow = pipe(setup, build, verify);
    expect(workflow.kind).toBe("Chain");
  });

  it("pipe rejects mismatched types", () => {
    // @ts-expect-error — type mismatch
    pipe(verify, setup);
  });

  it("pipe rejects unrelated types", () => {
    // @ts-expect-error — type mismatch
    pipe(deploy, setup);
  });

  it("pipe of single action returns that action", () => {
    const action = pipe(setup);
    // Single-action pipe returns the action directly (not wrapped in Chain)
    expect(action.kind).toBe("Invoke");
  });

  it("pipe right-folds into nested Chain nodes", () => {
    const action = pipe(setup, build, verify);
    // reduceRight: Chain(setup, Chain(build, verify))
    expect(action.kind).toBe("Chain");
    const outer = action as { kind: "Chain"; first: any; rest: any };
    expect(outer.first.kind).toBe("Invoke"); // setup
    expect(outer.rest.kind).toBe("Chain"); // Chain(build, verify)
    const inner = outer.rest as { kind: "Chain"; first: any; rest: any };
    expect(inner.first.kind).toBe("Invoke"); // build
    expect(inner.rest.kind).toBe("Invoke"); // verify
  });

  it("chain(a, b) produces Chain node", () => {
    const action = chain(setup, build);
    expect(action.kind).toBe("Chain");
    const node = action as { kind: "Chain"; first: any; rest: any };
    expect(node.first.kind).toBe("Invoke");
    expect(node.rest.kind).toBe("Invoke");
  });
});

// ---------------------------------------------------------------------------
// Execution tests
// ---------------------------------------------------------------------------

describe("pipe execution", () => {
  it("pipe of builtins: constant → getField", async () => {
    const result = await pipe(
      constant({ name: "alice", age: 30 }),
      getField("name"),
    ).run();
    expect(result).toBe("alice");
  });

  it("pipe of 4 builtins via postfix chaining", async () => {
    const result = await constant({ x: 42 })
      .getField("x")
      .wrapInField("value")
      .getField("value")
      .run();
    expect(result).toBe(42);
  });

  it("pipe with identity is passthrough", async () => {
    const result = await pipe(constant("hello"), identity()).run();
    expect(result).toBe("hello");
  });

  it("pipe with drop discards value", async () => {
    const result = await pipe(constant(42), drop).run();
    expect(result).toBeNull();
  });

  it(".then() postfix chains correctly", async () => {
    const result = await constant({ x: 10 }).then(constant(99)).run();
    expect(result).toBe(99);
  });

  it(".then() chains multiple steps", async () => {
    const result = await constant("first")
      .then(constant("second"))
      .then(constant("third"))
      .run();
    expect(result).toBe("third");
  });

  it("chain(a, b) executes equivalently to pipe(a, b)", async () => {
    const pipeResult = await pipe(constant({ a: 1 }), getField("a")).run();
    const chainResult = await chain(constant({ a: 1 }), getField("a")).run();
    expect(pipeResult).toBe(1);
    expect(chainResult).toBe(1);
    expect(pipeResult).toBe(chainResult);
  });

  it("large config (>200KB) works via config file", async () => {
    // This would hit E2BIG if passed as a CLI argument.
    const largePayload = "x".repeat(250_000);
    const result = await constant({ data: largePayload })
      .getField("data")
      .run();
    expect(result).toBe(largePayload);
  });

  it("tap runs side effect and passes input through", async () => {
    const result = await constant({ x: 42 }).tap(drop).run();
    expect(result).toEqual({ x: 42 });
  });

  it("tap standalone passes input through", async () => {
    const result = await pipe(
      constant("hello"),
      tap(constant("discarded")),
    ).run();
    expect(result).toBe("hello");
  });
});
