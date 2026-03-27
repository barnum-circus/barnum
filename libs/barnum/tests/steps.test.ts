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
  drop,
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
  it("self serializes as Root step and works in branches via drop()", () => {
    const cfg = configBuilder()
      .workflow((_steps, self) =>
        pipe(
          constant([{ file: "a.ts", message: "err" }]),
          classifyErrors(),
          branch({
            HasErrors: pipe(extractField("errors"), forEach(fix()), drop(), self),
            Clean: pipe(drop(), constant({ done: true })),
          }),
        ),
      );

    expect(cfg.workflow.kind).toBe("Pipe");
    const workflow = cfg.workflow as { kind: string; actions: unknown[] };
    const branchAction = workflow.actions[workflow.actions.length - 1] as {
      kind: string;
      cases: Record<string, { kind: string; actions: unknown[] }>;
    };
    const hasErrorsPipe = branchAction.cases.HasErrors;
    expect(hasErrorsPipe.actions[hasErrorsPipe.actions.length - 1]).toEqual({
      kind: "Step",
      step: { kind: "Root" },
    });
  });

  it("rejects piping a value directly into self", () => {
    configBuilder()
      .workflow((_steps, self) =>
        // @ts-expect-error — check outputs {valid: boolean} but self expects never
        pipe(constant({ result: "test" }), check(), self),
      );
  });
});

// -----------------------------------------------------------------------
// Mutual recursion via stepRef
// -----------------------------------------------------------------------

describe("mutual recursion", () => {
  it("stepRef enables cross-references between steps", () => {
    const cfg = configBuilder()
      .registerSteps(({ stepRef }) => ({
        A: pipe(check(), stepRef("B")),
        B: pipe(check(), stepRef("A")),
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
      step: { kind: "Named", name: "B" },
    });
    // B body ends with a Step reference to A
    const bBody = cfg.steps!.B as { kind: string; actions: unknown[] };
    expect(bBody.actions[bBody.actions.length - 1]).toEqual({
      kind: "Step",
      step: { kind: "Named", name: "A" },
    });
  });

  it("callback form provides typed access to previously registered steps", () => {
    const cfg = configBuilder()
      .registerSteps({ Setup: setup() })
      .registerSteps(({ steps }) => ({
        Pipeline: pipe(steps.Setup, process()),
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
      step: { kind: "Named", name: "Setup" },
    });
  });

  it("rejects invalid step references at compile time", () => {
    configBuilder()
      .registerSteps(({ stepRef }) => ({
        A: pipe(check(), stepRef("Bt")),
        B: pipe(check(), stepRef("A")),
      }))
      // @ts-expect-error — "Bt" is not a valid step name; return is error type
      .workflow((steps) => pipe(steps.A));
  });
});

// -----------------------------------------------------------------------
// Kitchen sink — exercises all features together
// -----------------------------------------------------------------------

describe("kitchen sink", () => {
  it("steps, stepRef, and self across registerSteps and workflow", () => {
    const cfg = configBuilder()
      // Batch 1: simple step, no cross-references (object form)
      .registerSteps({
        Setup: setup(),
      })
      // Batch 2: callback form — steps (prior batch) + stepRef (intra-batch)
      .registerSteps(({ steps, stepRef }) => ({
        Pipeline: pipe(
          steps.Setup,
          listFiles(),
          forEach(migrate()),
          stepRef("FixCycle"),
        ),
        FixCycle: loop(
          pipe(
            typeCheck(),
            classifyErrors(),
            branch({
              HasErrors: pipe(extractField("errors"), forEach(fix()), recur()),
              Clean: done(),
            }),
          ),
        ),
      }))
      // Workflow: steps (all registered) + self (root restart)
      .workflow((steps, self) =>
        pipe(
          constant({ project: "test" }),
          steps.Pipeline,
          classifyErrors(),
          branch({
            HasErrors: pipe(drop(), self),
            Clean: pipe(drop(), constant({ done: true })),
          }),
        ),
      );

    // All steps registered
    expect(cfg.steps).toHaveProperty("Setup");
    expect(cfg.steps).toHaveProperty("Pipeline");
    expect(cfg.steps).toHaveProperty("FixCycle");

    // Pipeline starts with steps.Setup reference
    const pipeline = cfg.steps!.Pipeline as { kind: string; actions: unknown[] };
    expect(pipeline.actions[0]).toEqual({
      kind: "Step",
      step: { kind: "Named", name: "Setup" },
    });

    // Pipeline ends with stepRef("FixCycle")
    expect(pipeline.actions[pipeline.actions.length - 1]).toEqual({
      kind: "Step",
      step: { kind: "Named", name: "FixCycle" },
    });

    // Workflow branch HasErrors ends with self (Root step)
    const workflowActions = (cfg.workflow as { actions: unknown[] }).actions;
    const branchAction = workflowActions[workflowActions.length - 1] as {
      kind: string;
      cases: Record<string, { kind: string; actions: unknown[] }>;
    };
    const hasErrorsActions = branchAction.cases.HasErrors.actions;
    expect(hasErrorsActions[hasErrorsActions.length - 1]).toEqual({
      kind: "Step",
      step: { kind: "Root" },
    });
  });
});
