import { describe, it, expect } from "vitest";
import {
  parallel,
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
// Named steps
// -----------------------------------------------------------------------

describe("named steps", () => {
  it("allows referencing registered steps", () => {
    const cfg = configBuilder()
      .registerSteps({ Finalize: finalize() })
      .workflow((steps) => pipe(constant({ valid: true }), steps.Finalize));
    expect(cfg.workflow.kind).toBe("Pipe");
  });

  it("rejects references to unregistered steps", () => {
    configBuilder()
      .registerSteps({ Finalize: finalize() })
      .workflow((steps) => {
        // @ts-expect-error — "Nonexistent" was never registered
        return steps.Nonexistent;
      });
  });

  it("supports multiple registerSteps calls", () => {
    const cfg = configBuilder()
      .registerSteps({ Finalize: finalize() })
      .registerSteps({ Revalidate: validate() })
      .workflow((steps) =>
        pipe(constant({ valid: true }), parallel(steps.Finalize, steps.Revalidate)),
      );
    expect(cfg.steps).toHaveProperty("Finalize");
    expect(cfg.steps).toHaveProperty("Revalidate");
  });

  it("uses named steps for a fix cycle", () => {
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
// Workflow self-reference
// -----------------------------------------------------------------------

describe("workflow self-reference", () => {
  it("workflow receives a self reference for recursion", () => {
    const cfg = configBuilder()
      .workflow((_steps, self) =>
        pipe(
          constant({ result: "test" }),
          check(),
          branch({
            Valid: finalize(),
            Invalid: self,
          }),
        ),
      );

    expect(cfg.workflow.kind).toBe("Pipe");
    const workflow = cfg.workflow as { kind: string; actions: unknown[] };
    const branchAction = workflow.actions[workflow.actions.length - 1] as {
      kind: string;
      cases: Record<string, unknown>;
    };
    expect(branchAction.cases.Invalid).toEqual({
      kind: "Step",
      step: "__self__",
    });
  });
});

// -----------------------------------------------------------------------
// Mutual recursion
// -----------------------------------------------------------------------

describe("mutual recursion", () => {
  it("registerSteps callback enables cross-references between steps", () => {
    const cfg = configBuilder()
      .registerSteps((refs) => ({
        A: pipe(check(), refs.B),
        B: pipe(check(), refs.A),
      }))
      .workflow((steps) =>
        pipe(constant({ result: "test" }), steps.A),
      );

    expect(cfg.steps).toHaveProperty("A");
    expect(cfg.steps).toHaveProperty("B");
    // A body ends with a Step reference to B
    const aBody = cfg.steps!.A as { kind: string; actions: unknown[] };
    expect(aBody.kind).toBe("Pipe");
    expect(aBody.actions[aBody.actions.length - 1]).toEqual({
      kind: "Step",
      step: "B",
    });
    // B body ends with a Step reference to A
    const bBody = cfg.steps!.B as { kind: string; actions: unknown[] };
    expect(bBody.actions[bBody.actions.length - 1]).toEqual({
      kind: "Step",
      step: "A",
    });
  });

  it("callback receives previously registered steps", () => {
    const cfg = configBuilder()
      .registerSteps({ Setup: setup() })
      .registerSteps((refs) => ({
        Pipeline: pipe(refs.Setup, process()),
      }))
      .workflow((steps) =>
        pipe(constant({ project: "test" }), steps.Pipeline),
      );

    expect(cfg.steps).toHaveProperty("Setup");
    expect(cfg.steps).toHaveProperty("Pipeline");
    // Pipeline body starts with a Step reference to Setup
    const pipelineBody = cfg.steps!.Pipeline as { kind: string; actions: unknown[] };
    expect(pipelineBody.actions[0]).toEqual({
      kind: "Step",
      step: "Setup",
    });
  });
});
