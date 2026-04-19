import { describe, it, expect } from "vitest";
import {
  type ExtractInput,
  type ExtractOutput,
  pipe,
  branch,
  config,
} from "../src/ast.js";
import { constant, drop, getField, identity } from "../src/builtins/index.js";
import { chain } from "../src/chain.js";
import { runPipeline } from "../src/run.js";
import { setup, build, verify, deploy, classifyErrors } from "./handlers.js";

// ---------------------------------------------------------------------------
// Type assertion helpers (compile-time only)
// ---------------------------------------------------------------------------

type IsExact<T, U> = [T] extends [U] ? ([U] extends [T] ? true : false) : false;

// eslint-disable-next-line @typescript-eslint/no-unused-vars
function assertExact<_T extends true>(): void {}

// ---------------------------------------------------------------------------
// Type tests
// ---------------------------------------------------------------------------

describe("pipe type tests", () => {
  it("pipe: input of first, output of last", () => {
    const action = pipe(setup, build, verify);
    assertExact<IsExact<ExtractInput<typeof action>, { project: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { verified: boolean }>>();
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
    assertExact<IsExact<ExtractInput<typeof action>, any>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { deployed: boolean }>>();
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
    const result = await runPipeline(
      pipe(constant({ name: "alice", age: 30 }), getField("name")),
    );
    expect(result).toBe("alice");
  });

  it("pipe of 4 builtins via postfix chaining", async () => {
    const result = await runPipeline(
      constant({ x: 42 }).getField("x").wrapInField("value").getField("value"),
    );
    expect(result).toBe(42);
  });

  it("pipe with identity is passthrough", async () => {
    const result = await runPipeline(pipe(constant("hello"), identity()));
    expect(result).toBe("hello");
  });

  it("pipe with drop discards value", async () => {
    const result = await runPipeline(pipe(constant(42), drop));
    expect(result).toBeNull();
  });

  it(".then() postfix chains correctly", async () => {
    const result = await runPipeline(constant({ x: 10 }).then(constant(99)));
    expect(result).toBe(99);
  });

  it(".then() chains multiple steps", async () => {
    const result = await runPipeline(
      constant("first").then(constant("second")).then(constant("third")),
    );
    expect(result).toBe("third");
  });

  it("chain(a, b) executes equivalently to pipe(a, b)", async () => {
    const pipeResult = await runPipeline(
      pipe(constant({ a: 1 }), getField("a")),
    );
    const chainResult = await runPipeline(
      chain(constant({ a: 1 }), getField("a")),
    );
    expect(pipeResult).toBe(1);
    expect(chainResult).toBe(1);
    expect(pipeResult).toBe(chainResult);
  });
});
