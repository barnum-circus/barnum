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
  drop,
  extractField,
  identity,
  merge,
  recur,
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
} from "./handlers.js";

// -----------------------------------------------------------------------
// Pipe
// -----------------------------------------------------------------------

describe("pipe", () => {
  it("chains setup → build → verify → deploy", () => {
    const cfg = config(
      pipe(
        constant({ project: "test" }),
        setup(),
        build(),
        verify(),
        deploy(),
      ),
    );
    expect(cfg.workflow.kind).toBe("Pipe");
  });

  it("rejects mismatched types", () => {
    // verify outputs { verified: boolean }, setup expects { project: string }
    // @ts-expect-error — type mismatch between verify's output and setup's input
    pipe(verify(), setup());
  });

  it("chains three steps correctly", () => {
    const workflow = pipe(setup(), build(), verify());
    expect(workflow.kind).toBe("Pipe");
  });

  it("rejects unrelated types", () => {
    // deploy outputs { deployed: true }, setup expects { project: string }
    // @ts-expect-error — type mismatch between deploy's output and setup's input
    pipe(deploy(), setup());
  });
});

// -----------------------------------------------------------------------
// ForEach
// -----------------------------------------------------------------------

describe("forEach", () => {
  it("maps input/output to arrays", () => {
    const workflow = forEach(verify());
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
    const workflow = parallel(verify(), verify());
    expect(workflow.kind).toBe("Parallel");
  });

  it("rejects actions with different input types", () => {
    // setup expects { project: string }, verify expects { artifact: string }
    // @ts-expect-error — input types do not unify
    parallel(setup(), verify());
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
              Ok: build(),
              Err: build(),
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
      Yes: deploy(),
      No: deploy(),
    });
    expect(workflow.kind).toBe("Branch");
  });

  it("rejects output flowing into incompatible step", () => {
    // branch outputs { deployed: true }, but setup expects { project: string }
    // @ts-expect-error — branch output doesn't satisfy next step's input
    pipe(branch({ A: deploy(), B: deploy() }), setup());
  });
});

// -----------------------------------------------------------------------
// Loop
// -----------------------------------------------------------------------

describe("loop", () => {
  it("accepts body returning LoopResult", () => {
    const workflow = loop(healthCheck());
    expect(workflow.kind).toBe("Loop");
  });

  it("rejects body not returning LoopResult", () => {
    // verify: { artifact: string } → { verified: boolean } — not a LoopResult
    // @ts-expect-error — loop body must return LoopResult<In, Out>
    loop(verify());
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
            drop(),
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
    const wrapped = attempt(verify());
    expect(wrapped.kind).toBe("Attempt");
  });

  it("chains in pipe with result-aware consumer", () => {
    const workflow = pipe(build(), attempt(verify()));
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
        parallel(identity(), build()),
        merge(),
      ),
    );
    expect(cfg.workflow.kind).toBe("Pipe");
  });
});
