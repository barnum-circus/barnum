import { describe, it, expect } from "vitest";
import {
  all,
  attempt,
  config,
  configBuilder,
  loop,
  matchCases,
  sequence,
  traverse,
} from "../src/core.js";
import {
  constant,
  done,
  extractField,
  identity,
  merge,
  recur,
} from "../src/builtins.js";

import setup from "./handlers/setup.js";
import process_ from "./handlers/process.js";
import check from "./handlers/check.js";
import finalize from "./handlers/finalize.js";
import listFiles from "./handlers/list-files.js";
import migrate from "./handlers/migrate.js";
import typeCheck from "./handlers/type-check.js";
import classifyErrors from "./handlers/classify-errors.js";
import fix from "./handlers/fix.js";

// -----------------------------------------------------------------------
// Pattern 1: Linear pipeline
// -----------------------------------------------------------------------

describe("linear pipeline", () => {
  it("chains setup → process → check → finalize", () => {
    const cfg = config(
      sequence(
        constant({ project: "test" }),
        setup(),
        process_(),
        check(),
        finalize(),
      ),
    );
    expect(cfg.workflow.kind).toBe("Sequence");
  });
});

// -----------------------------------------------------------------------
// Pattern 2: Fan-out with traverse
// -----------------------------------------------------------------------

describe("fan-out with traverse", () => {
  it("setup → listFiles → traverse(migrate)", () => {
    const cfg = config(
      sequence(
        constant({ project: "test" }),
        setup(),
        listFiles(),
        traverse(migrate()),
      ),
    );
    expect(cfg.workflow.kind).toBe("Sequence");
  });
});

// -----------------------------------------------------------------------
// Pattern 3: Type-check loop (from WORKFLOW_ALGEBRA.md example 3)
//
// typeCheck → classifyErrors → match {
//   HasErrors: extractField("errors") → traverse(fix) → recur()
//   Clean: done()
// }
// -----------------------------------------------------------------------

describe("type-check loop", () => {
  it("loops until clean", () => {
    const cfg = config(
      sequence(
        constant({ project: "test" }),
        setup(),
        listFiles(),
        traverse(migrate()),
        loop(
          sequence(
            typeCheck(),
            classifyErrors(),
            matchCases({
              HasErrors: sequence(
                extractField("errors"),
                traverse(fix()),
                recur(),
              ),
              Clean: done(),
            }),
          ),
        ),
      ),
    );
    expect(cfg.workflow.kind).toBe("Sequence");
  });
});

// -----------------------------------------------------------------------
// Pattern 4: Parallel branches with error handling
//
// all(
//   fetchA,
//   sequence(attempt(fetchB), match { Ok: extractField, Err: default })
// )
// -----------------------------------------------------------------------

describe("parallel branches with error handling", () => {
  it("runs branches in parallel with attempt/match fallback", () => {
    const cfg = config(
      sequence(
        constant({ project: "test" }),
        all(
          setup(),
          sequence(
            attempt(setup()),
            matchCases({
              Ok: process_(),
              Err: process_(),
            }),
          ),
        ),
      ),
    );
    expect(cfg.workflow.kind).toBe("Sequence");
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
          sequence(
            typeCheck(),
            classifyErrors(),
            matchCases({
              HasErrors: sequence(
                extractField("errors"),
                traverse(fix()),
                recur(),
              ),
              Clean: done(),
            }),
          ),
        ),
      })
      .workflow((steps) =>
        sequence(
          constant({ project: "test" }),
          setup(),
          listFiles(),
          traverse(migrate()),
          steps.FixCycle,
        ),
      );
    expect(cfg.workflow.kind).toBe("Sequence");
    expect(cfg.steps).toHaveProperty("FixCycle");
  });

  it("uses multiple registerSteps calls to reference earlier steps", () => {
    const cfg = configBuilder()
      .registerSteps({
        Migrate: sequence(listFiles(), traverse(migrate())),
      })
      .registerSteps({
        FixCycle: loop(
          sequence(
            typeCheck(),
            classifyErrors(),
            matchCases({
              HasErrors: sequence(
                extractField("errors"),
                traverse(fix()),
                recur(),
              ),
              Clean: done(),
            }),
          ),
        ),
      })
      .workflow((steps) =>
        sequence(constant({ project: "test" }), setup(), steps.Migrate, steps.FixCycle),
      );
    expect(cfg.steps).toHaveProperty("Migrate");
    expect(cfg.steps).toHaveProperty("FixCycle");
  });
});

// -----------------------------------------------------------------------
// Pattern 6: Reader monad (user-land context passing)
//
// all(identity(), handler) → merge()
// Preserves the original input alongside the handler's output.
// -----------------------------------------------------------------------

describe("reader monad pattern", () => {
  it("preserves context via all + identity + merge", () => {
    const cfg = config(
      sequence(
        constant({ initialized: true, project: "test" }),
        all(identity(), process_()),
        merge(),
      ),
    );
    expect(cfg.workflow.kind).toBe("Sequence");
  });
});
