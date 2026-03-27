import { describe, it, expect } from "vitest";
import {
  parallel,
  attempt,
  config,
  configBuilder,
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
  listFiles,
  migrate,
  typeCheck,
  classifyErrors,
  fix,
} from "./handlers.js";

// -----------------------------------------------------------------------
// Pattern 1: Linear pipeline
// -----------------------------------------------------------------------

describe("linear pipeline", () => {
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
});

// -----------------------------------------------------------------------
// Pattern 2: Fan-out with forEach
// -----------------------------------------------------------------------

describe("fan-out with forEach", () => {
  it("setup → listFiles → forEach(migrate)", () => {
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
// Pattern 3: Type-check loop (from WORKFLOW_ALGEBRA.md example 3)
//
// typeCheck → classifyErrors → branch {
//   HasErrors: extractField("errors") → forEach(fix) → recur()
//   Clean: done()
// }
// -----------------------------------------------------------------------

describe("type-check loop", () => {
  it("loops until clean", () => {
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
// Pattern 4: Parallel branches with error handling
//
// parallel(
//   fetchA,
//   pipe(attempt(fetchB), branch { Ok: extractField, Err: default })
// )
// -----------------------------------------------------------------------

describe("parallel branches with error handling", () => {
  it("runs branches in parallel with attempt/branch fallback", () => {
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
// Pattern 5: Named steps — linter workflow
//
// Fan out to individual files, type-check, fix loop, finalize.
// Uses registerSteps for the fix loop.
// -----------------------------------------------------------------------

describe("named steps — linter workflow", () => {
  it("uses named steps for the fix cycle", () => {
    const cfg = configBuilder()
      .registerSteps({
        FixCycle: loop(
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
      })
      .workflow((steps) =>
        pipe(
          constant({ project: "test" }),
          setup(),
          listFiles(),
          forEach(migrate()),
          steps.FixCycle,
        ),
      );
    expect(cfg.workflow.kind).toBe("Pipe");
    expect(cfg.steps).toHaveProperty("FixCycle");
  });

  it("uses multiple registerSteps calls to reference earlier steps", () => {
    const cfg = configBuilder()
      .registerSteps({
        Migrate: pipe(listFiles(), forEach(migrate())),
      })
      .registerSteps({
        FixCycle: loop(
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
      })
      .workflow((steps) =>
        pipe(constant({ project: "test" }), setup(), steps.Migrate, steps.FixCycle),
      );
    expect(cfg.steps).toHaveProperty("Migrate");
    expect(cfg.steps).toHaveProperty("FixCycle");
  });
});

// -----------------------------------------------------------------------
// Pattern 6: Reader monad (user-land context passing)
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
