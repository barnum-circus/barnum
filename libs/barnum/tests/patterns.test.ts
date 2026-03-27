import { describe, it, expect } from "vitest";
import {
  parallel,
  attempt,
  config,
  loop,
  branch,
  pipe,
  forEach,
} from "../src/ast.js";
import {
  constant,
  done,
  extractField,
  identity,
  merge,
  recur,
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

// -----------------------------------------------------------------------
// Pipe
// -----------------------------------------------------------------------

describe("pipe", () => {
  it("chains setup → process → check → finalize", () => {
    const cfg = config(
      pipe(
        constant({ project: "test" }),
        setup(),
        process(),
        check(),
        finalize(),
      ),
    );
    expect(cfg.workflow.kind).toBe("Pipe");
  });

  it("rejects mismatched types", () => {
    // CheckOutput ({ valid: boolean }) does not match SetupInput ({ project: string })
    // @ts-expect-error — type mismatch between check's output and setup's input
    pipe(check(), setup());
  });

  it("chains three steps correctly", () => {
    const workflow = pipe(setup(), process(), check());
    expect(workflow.kind).toBe("Pipe");
  });

  it("rejects unrelated types", () => {
    // FinalizeOutput ({ done: true }) does not match SetupInput ({ project: string })
    // @ts-expect-error — type mismatch between finalize's output and setup's input
    pipe(finalize(), setup());
  });
});

// -----------------------------------------------------------------------
// ForEach
// -----------------------------------------------------------------------

describe("forEach", () => {
  it("maps input/output to arrays", () => {
    const workflow = forEach(check());
    expect(workflow.kind).toBe("ForEach");
  });

  it("composes with pipe: setup → listFiles → forEach(migrate)", () => {
    const cfg = config(
      pipe(
        constant({ project: "test" }),
        setup(),
        listFiles(),
        forEach(migrate()),
      ),
    );
    expect(cfg.workflow.kind).toBe("Pipe");
  });
});

// -----------------------------------------------------------------------
// Parallel
// -----------------------------------------------------------------------

describe("parallel", () => {
  it("accepts actions with the same input type", () => {
    const workflow = parallel(check(), check());
    expect(workflow.kind).toBe("Parallel");
  });

  it("rejects actions with different input types", () => {
    // setup expects { project: string }, check expects { result: string }
    // @ts-expect-error — input types do not unify
    parallel(setup(), check());
  });

  it("composes with error handling", () => {
    const cfg = config(
      pipe(
        constant({ project: "test" }),
        parallel(
          setup(),
          pipe(
            attempt(setup()),
            branch({
              Ok: process(),
              Err: process(),
            }),
          ),
        ),
      ),
    );
    expect(cfg.workflow.kind).toBe("Pipe");
  });
});

// -----------------------------------------------------------------------
// Branch
// -----------------------------------------------------------------------

describe("branch", () => {
  it("accepts cases with the same output type", () => {
    const workflow = branch({
      Yes: finalize(),
      No: finalize(),
    });
    expect(workflow.kind).toBe("Branch");
  });

  it("rejects output flowing into incompatible step", () => {
    // branch outputs { done: true }, but setup expects { project: string }
    // @ts-expect-error — branch output doesn't satisfy next step's input
    pipe(branch({ A: finalize(), B: finalize() }), setup());
  });
});

// -----------------------------------------------------------------------
// Loop
// -----------------------------------------------------------------------

describe("loop", () => {
  it("accepts body returning LoopResult", () => {
    const workflow = loop(validate());
    expect(workflow.kind).toBe("Loop");
  });

  it("rejects body not returning LoopResult", () => {
    // check: { result: string } → { valid: boolean } — not a LoopResult
    // @ts-expect-error — loop body must return LoopResult<In, Out>
    loop(check());
  });

  it("composes type-check loop with branch", () => {
    const cfg = config(
      pipe(
        constant({ project: "test" }),
        setup(),
        listFiles(),
        forEach(migrate()),
        loop(
          pipe(
            typeCheck(),
            classifyErrors(),
            branch({
              HasErrors: pipe(
                extractField("errors"),
                forEach(fix()),
                recur(),
              ),
              Clean: done(),
            }),
          ),
        ),
      ),
    );
    expect(cfg.workflow.kind).toBe("Pipe");
  });
});

// -----------------------------------------------------------------------
// Attempt
// -----------------------------------------------------------------------

describe("attempt", () => {
  it("wraps output in AttemptResult", () => {
    const wrapped = attempt(check());
    expect(wrapped.kind).toBe("Attempt");
  });

  it("chains in pipe with result-aware consumer", () => {
    const workflow = pipe(process(), attempt(check()));
    expect(workflow.kind).toBe("Pipe");
  });
});

// -----------------------------------------------------------------------
// Reader monad pattern
//
// parallel(identity(), handler) → merge()
// Preserves the original input alongside the handler's output.
// -----------------------------------------------------------------------

describe("reader monad pattern", () => {
  it("preserves context via parallel + identity + merge", () => {
    const cfg = config(
      pipe(
        constant({ initialized: true, project: "test" }),
        parallel(identity(), process()),
        merge(),
      ),
    );
    expect(cfg.workflow.kind).toBe("Pipe");
  });
});
