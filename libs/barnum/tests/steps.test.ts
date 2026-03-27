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
      .workflow(({ steps }) => pipe(constant({ valid: true }), steps.Finalize));
    expect(cfg.workflow.kind).toBe("Pipe");
  });

  it("rejects references to unregistered steps", () => {
    configBuilder()
      .registerSteps({ Finalize: finalize() })
      .workflow(({ steps }) => {
        // @ts-expect-error — "Nonexistent" was never registered
        return steps.Nonexistent;
      });
  });

  it("supports multiple registerSteps calls", () => {
    const cfg = configBuilder()
      .registerSteps({ Finalize: finalize() })
      .registerSteps({ Revalidate: validate() })
      .workflow(({ steps }) =>
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
      })
      .workflow(({ steps }) =>
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
      })
      .workflow(({ steps }) =>
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
      .workflow(({ self }) =>
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
    const branchAction = workflow.actions.at(-1) as {
      kind: string;
      cases: Record<string, { kind: string; actions: unknown[] }>;
    };
    const hasErrorsPipe = branchAction.cases.HasErrors;
    expect(hasErrorsPipe.actions.at(-1)).toEqual({
      kind: "Step",
      step: { kind: "Root" },
    });
  });

  it("rejects piping a value directly into self", () => {
    configBuilder()
      .workflow(({ self }) =>
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
      .workflow(({ steps }) =>
        pipe(constant({ result: "test" }), steps.A),
      );

    expect(cfg.steps).toHaveProperty("A");
    expect(cfg.steps).toHaveProperty("B");
    // A body ends with a Step reference to B
    const aBody = cfg.steps!.A as { kind: string; actions: unknown[] };
    expect(aBody.kind).toBe("Pipe");
    expect(aBody.actions.at(-1)).toEqual({
      kind: "Step",
      step: { kind: "Named", name: "B" },
    });
    // B body ends with a Step reference to A
    const bBody = cfg.steps!.B as { kind: string; actions: unknown[] };
    expect(bBody.actions.at(-1)).toEqual({
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
      .workflow(({ steps }) =>
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
      .workflow(({ steps }) => pipe(steps.A));
  });
});

// -----------------------------------------------------------------------
// Showcase — mutual recursion between two steps
// -----------------------------------------------------------------------
//
// A type-check → fix cycle using two steps that reference each other.
// TypeCheck discovers errors and jumps to FixAll. FixAll patches each
// file and jumps back to TypeCheck. The cycle continues until clean.

describe("showcase: type-check ↔ fix cycle", () => {
  it("two steps reference each other via stepRef", () => {
    const cfg = configBuilder()
      .registerSteps(({ stepRef }) => ({
        TypeCheck: pipe(
          drop(),
          typeCheck(),
          classifyErrors(),
          branch({
            HasErrors: stepRef("FixAll"),
            Clean: drop(),
          }),
        ),
        FixAll: pipe(
          forEach(fix()),
          drop(),
          stepRef("TypeCheck"),
        ),
      }))
      .workflow(({ steps }) =>
        pipe(
          constant({ project: "my-app" }),
          setup(),
          listFiles(),
          forEach(migrate()),
          drop(),
          steps.TypeCheck,
        ),
      );

    expect(cfg.steps).toHaveProperty("TypeCheck");
    expect(cfg.steps).toHaveProperty("FixAll");
    expect(cfg.workflow.kind).toBe("Pipe");
  });
});

// -----------------------------------------------------------------------
// Kitchen sink — a migration workflow that exercises every feature
// -----------------------------------------------------------------------
//
// Scenario: a codebase migration tool that:
//   1. Sets up the project environment           (registered step, batch 1)
//   2. Lists and migrates all source files        (registered step, batch 2 — refs batch 1 via `steps`)
//   3. Runs a type-check → fix loop               (registered step, batch 2 — mutual recursion via `stepRef`)
//   4. Orchestrates everything in a workflow       (uses `steps` for registered steps, `self` for restart)
//
// Features demonstrated:
//   - Object-form registerSteps (batch 1)
//   - Callback-form registerSteps with `steps` (cross-batch) and `stepRef` (intra-batch)
//   - Workflow with `steps` and `self`

describe("kitchen sink", () => {
  it("migration workflow: setup → migrate → fix cycle, restart on failure", () => {
    const cfg = configBuilder()
      // ── Batch 1 (object form): standalone steps ──
      .registerSteps({
        Setup: setup(),
      })
      // ── Batch 2 (callback form): cross-batch refs + mutual recursion ──
      .registerSteps(({ steps, stepRef }) => ({
        // MigrateAll chains through the previously registered Setup step
        MigrateAll: pipe(
          steps.Setup,
          listFiles(),
          forEach(migrate()),
          stepRef("FixCycle"), // jump to the fix cycle defined below
        ),
        // FixCycle: type-check, classify errors, fix or finish
        FixCycle: loop(
          pipe(
            drop(),
            typeCheck(),
            classifyErrors(),
            branch({
              HasErrors: pipe(extractField("errors"), forEach(fix()), recur()),
              Clean: done(),
            }),
          ),
        ),
      }))
      // ── Workflow: orchestrate with self-restart on persistent errors ──
      .workflow(({ steps, self }) =>
        pipe(
          constant({ project: "my-app" }),
          steps.MigrateAll,
          classifyErrors(),
          branch({
            HasErrors: pipe(drop(), self),  // restart the entire workflow
            Clean: pipe(drop(), constant({ migrated: true })),
          }),
        ),
      );

    // ── Verify structure ──

    // All three steps registered
    expect(cfg.steps).toHaveProperty("Setup");
    expect(cfg.steps).toHaveProperty("MigrateAll");
    expect(cfg.steps).toHaveProperty("FixCycle");

    // MigrateAll starts with a reference to Setup (cross-batch via `steps`)
    const migrateAll = cfg.steps!.MigrateAll as { kind: string; actions: unknown[] };
    expect(migrateAll.actions[0]).toEqual({
      kind: "Step",
      step: { kind: "Named", name: "Setup" },
    });

    // MigrateAll ends with a reference to FixCycle (intra-batch via `stepRef`)
    expect(migrateAll.actions.at(-1)).toEqual({
      kind: "Step",
      step: { kind: "Named", name: "FixCycle" },
    });

    // Workflow's HasErrors branch ends with self (Root step)
    const workflowActions = (cfg.workflow as { actions: unknown[] }).actions;
    const branchAction = workflowActions.at(-1) as {
      kind: string;
      cases: Record<string, { kind: string; actions: unknown[] }>;
    };
    expect(branchAction.cases.HasErrors.actions.at(-1)).toEqual({
      kind: "Step",
      step: { kind: "Root" },
    });
  });
});
