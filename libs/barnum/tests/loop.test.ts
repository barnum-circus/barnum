import { beforeEach, describe, expect, it } from "vitest";
import {
  config,
  earlyReturn,
  forEach,
  loop,
  pipe,
  recur as recurCombinator,
  resetEffectIdCounter,
} from "../src/ast.js";
import { constant, drop, splitFirst } from "../src/builtins/index.js";
import { runPipeline } from "../src/run.js";
import {
  classifyErrors,
  fix,
  healthCheck,
  listFiles,
  migrate,
  setup,
  typeCheck,
} from "./handlers.js";
import { assertIO } from "./type-utils.js";

function expectedTagAst(kind: string) {
  return {
    kind: "Chain",
    first: {
      kind: "All",
      actions: [
        {
          kind: "Chain",
          first: {
            kind: "Invoke",
            handler: {
              kind: "Builtin",
              builtin: { kind: "Constant", value: kind },
            },
          },
          rest: {
            kind: "Invoke",
            handler: {
              kind: "Builtin",
              builtin: { kind: "WrapInField", field: "kind" },
            },
          },
        },
        {
          kind: "Invoke",
          handler: {
            kind: "Builtin",
            builtin: { kind: "WrapInField", field: "value" },
          },
        },
      ],
    },
    rest: {
      kind: "Invoke",
      handler: { kind: "Builtin", builtin: { kind: "Merge" } },
    },
  };
}

// ---------------------------------------------------------------------------
// Type tests
// ---------------------------------------------------------------------------

describe("loop type tests", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("loop: input matches Continue type, output is Break type", () => {
    const action = loop<{ stable: true }, { deployed: boolean }>(
      (recur, done) => healthCheck.branch({ Continue: recur, Break: done }),
    );
    assertIO<typeof action, { deployed: boolean }, { stable: true }>();
    expect(action.kind).toBe("Chain");
  });

  it("loop with branch/recur/done: output is null with void defaults", () => {
    const action = loop((recur, done) =>
      pipe(typeCheck, classifyErrors).branch({
        HasErrors: pipe(forEach(fix).drop(), recur),
        Clean: done,
      }),
    );
    assertIO<typeof action, any, null>();
    expect(action.kind).toBe("Chain");
  });

  it("loop with done: zero type params (terminate pattern)", () => {
    const action = loop((recur, done) =>
      pipe(typeCheck, classifyErrors).branch({
        HasErrors: pipe(forEach(fix).drop(), recur),
        Clean: done,
      }),
    );
    assertIO<typeof action, any, null>();
  });

  it("loop<TBreak, TIn>: both explicit for stateful loops", () => {
    const action = loop<{ stable: true }, { deployed: boolean }>(
      (recur, done) => healthCheck.branch({ Continue: recur, Break: done }),
    );
    assertIO<typeof action, { deployed: boolean }, { stable: true }>();
  });

  it("without explicit TBreak, done has input=null (accepts void variants)", () => {
    loop((recur, done) => {
      assertIO<typeof done, null, never>();
      classifyErrors.branch({ HasErrors: forEach(fix), Clean: done });
      return recur;
    });
  });

  it("without explicit TBreak, done has input=null (rejects non-null)", () => {
    loop((recur, done) => {
      // @ts-expect-error — done: TypedAction<null, never> can't accept { stable: true } from Break
      healthCheck.branch({ Continue: drop, Break: done });
      return recur;
    });
  });

  it("done and recur both output never", () => {
    loop<{ stable: true }, { deployed: boolean }>((recur, done) => {
      assertIO<typeof recur, { deployed: boolean }, never>();
      assertIO<typeof done, { stable: true }, never>();
      return healthCheck.branch({ Continue: recur, Break: done });
    });
  });

  it("loop with TIn=void has any input", () => {
    const action = loop((recur, done) =>
      pipe(typeCheck, classifyErrors).branch({
        HasErrors: pipe(forEach(fix).drop(), recur),
        Clean: done,
      }),
    );
    assertIO<typeof action, any, null>();
  });

  it("loop with explicit TIn has exact input", () => {
    const action = loop<{ stable: true }, { deployed: boolean }>(
      (recur, done) => healthCheck.branch({ Continue: recur, Break: done }),
    );
    assertIO<typeof action, { deployed: boolean }, { stable: true }>();
  });

  it(".drop() before recur connects void output to void input", () => {
    loop((recur, done) =>
      pipe(typeCheck, classifyErrors).branch({
        HasErrors: pipe(forEach(fix).drop(), recur),
        Clean: done,
      }),
    );
  });
});

// ---------------------------------------------------------------------------
// AST structure tests
// ---------------------------------------------------------------------------

describe("loop AST structure", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("loop produces Chain(tag(Continue), RestartHandle(...)) AST", () => {
    const workflow = loop<{ stable: true }, { deployed: boolean }>(
      (recur, done) => healthCheck.branch({ Continue: recur, Break: done }),
    );
    expect(workflow.kind).toBe("Chain");
    const chain = workflow as any;
    // First: tag("Continue")
    expect(chain.first).toEqual(expectedTagAst("LoopResult.Continue"));
    // Rest: RestartHandle
    expect(chain.rest.kind).toBe("RestartHandle");
    expect(typeof chain.rest.restart_handler_id).toBe("number");
    expect(chain.rest.body.kind).toBe("Branch");
    expect(Object.keys(chain.rest.body.cases).toSorted()).toEqual([
      "Break",
      "Continue",
    ]);
  });

  it("loop composes type-check loop with branch", () => {
    const cfg = config(
      pipe(
        constant({ project: "test" }),
        setup,
        listFiles,
        forEach(migrate),
      ).then(
        loop((recur, done) =>
          pipe(typeCheck, classifyErrors).branch({
            HasErrors: pipe(forEach(fix).drop(), recur),
            Clean: done,
          }),
        ),
      ),
    );
    expect(cfg.workflow.kind).toBe("Chain");
  });
});

// ---------------------------------------------------------------------------
// Execution tests
// ---------------------------------------------------------------------------

describe("loop execution", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("loop that immediately breaks returns break value", async () => {
    const result = await runPipeline(
      loop<number>((_, done) => pipe(constant(42), done)),
    );
    expect(result).toBe(42);
  });

  it("loop iterates then breaks (splitFirst countdown)", async () => {
    // State: number[]. Each iter: splitFirst. Some → recur with tail. None → break.
    const result = await runPipeline(
      pipe(
        constant([1, 2, 3]),
        loop<string, Array<number>>((recur, done) =>
          splitFirst<number>().branch({
            Some: pipe(drop, constant([] as Array<number>), recur),
            None: pipe(constant("done"), done),
          }),
        ),
      ),
    );
    expect(result).toBe("done");
  });
});

describe("earlyReturn execution", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("earlyReturn exits early with value", async () => {
    const result = await runPipeline(
      earlyReturn<string>((ret) => pipe(constant("early"), ret)),
    );
    expect(result).toBe("early");
  });

  it("earlyReturn completes normally without early return", async () => {
    const result = await runPipeline(
      earlyReturn<string, any, number>((_ret) => constant(42)),
    );
    expect(result).toBe(42);
  });
});

describe("recur execution", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("recur restarts body with new input", async () => {
    // State: number[]. First iter: has elements → restart with []. Second iter: empty → return "done".
    const result = await runPipeline(
      pipe(
        constant([1, 2]),
        recurCombinator<Array<number>, string>((restart) =>
          splitFirst<number>().branch({
            Some: pipe(drop, constant([] as Array<number>), restart),
            None: constant("done"),
          }),
        ),
      ),
    );
    expect(result).toBe("done");
  });

  it("recur completes immediately without restart", async () => {
    const result = await runPipeline(
      recurCombinator<void, string>((_restart) => constant("immediate")),
    );
    expect(result).toBe("immediate");
  });
});
