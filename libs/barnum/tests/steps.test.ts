import { describe, it, expect } from "vitest";
import {
  type Action,
  all,
  workflowBuilder,
  loop,
  branch,
  pipe,
  forEach,
} from "../src/ast.js";
import {
  constant,
  drop,
} from "../src/builtins.js";

/**
 * Type-narrowing assertion for Action's discriminated union.
 * Narrows `action` to the specific variant matching `kind`.
 */
function assertKind<T extends Action, K extends Action["kind"]>(
  action: T,
  kind: K,
): asserts action is T & Extract<Action, { kind: K }> {
  expect(action.kind).toBe(kind);
}

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
  type ClassifyResult,
  type TypeError,
} from "./handlers.js";

type HasErrors = Extract<ClassifyResult, { kind: "HasErrors" }>;
type Clean = Extract<ClassifyResult, { kind: "Clean" }>;

// -----------------------------------------------------------------------
// Named steps
// -----------------------------------------------------------------------

describe("named steps", () => {
  it("allows referencing registered steps", () => {
    const cfg = workflowBuilder()
      .registerSteps({ Deploy: deploy })
      .workflow(({ steps }) => pipe(constant({ verified: true }), steps.Deploy));
    expect(cfg.workflow.kind).toBe("Chain");
  });

  it("rejects references to unregistered steps", () => {
    workflowBuilder()
      .registerSteps({ Deploy: deploy })
      .workflow(({ steps }) => {
        // @ts-expect-error — "Nonexistent" was never registered
        return steps.Nonexistent;
      });
  });

  it("supports multiple registerSteps calls", () => {
    const cfg = workflowBuilder()
      .registerSteps({ Deploy: deploy })
      .registerSteps({ HealthCheck: healthCheck })
      .workflow(({ steps }) =>
        pipe(
          constant({ verified: true }),
          steps.Deploy,
        ).then(loop<{ stable: true }, { deployed: boolean }>((recur, done) =>
          steps.HealthCheck.branch({ Continue: recur, Break: done }),
        )),
      );
    expect(cfg.steps).toHaveProperty("Deploy");
    expect(cfg.steps).toHaveProperty("HealthCheck");
  });

  it("uses named steps for a fix cycle", () => {
    const cfg = workflowBuilder()
      .registerSteps({
        FixCycle: loop<void>((recur, done) =>
          pipe(typeCheck, classifyErrors).branch({
            HasErrors: pipe(forEach(fix).drop(), recur),
            Clean: done,
          }),
        ),
      })
      .workflow(({ steps }) =>
        pipe(
          constant({ project: "test" }),
          setup,
          listFiles,
          forEach(migrate),
          steps.FixCycle,
        ),
      );
    expect(cfg.workflow.kind).toBe("Chain");
    expect(cfg.steps).toHaveProperty("FixCycle");
  });

  it("uses multiple registerSteps calls to reference earlier steps", () => {
    const cfg = workflowBuilder()
      .registerSteps({
        Migrate: pipe(listFiles, forEach(migrate)),
      })
      .registerSteps({
        FixCycle: loop<void>((recur, done) =>
          pipe(typeCheck, classifyErrors).branch({
            HasErrors: pipe(forEach(fix).drop(), recur),
            Clean: done,
          }),
        ),
      })
      .workflow(({ steps }) =>
        pipe(constant({ project: "test" }), setup, steps.Migrate, steps.FixCycle),
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
    const cfg = workflowBuilder()
      .workflow(({ self }) =>
        pipe(
          constant([{ file: "a.ts", message: "err" }]),
          classifyErrors,
          branch({
            HasErrors: pipe(forEach(fix), drop<any>(), self),
            Clean: pipe(drop<void>(), constant({ done: true })),
          }),
        ),
      );

    // pipe(constant, classifyErrors, branch) → Chain(constant, Chain(classifyErrors, branch))
    assertKind(cfg.workflow, "Chain");
    assertKind(cfg.workflow.rest, "Chain");
    assertKind(cfg.workflow.rest.rest, "Branch");
    // HasErrors case: auto-unwrap inserts ExtractField("value") before the handler.
    // Handler: pipe(forEach(fix), drop, self)
    // Full: Chain(ExtractField("value"), Chain(forEach(fix), Chain(drop, self)))
    const hasErrors = cfg.workflow.rest.rest.cases.HasErrors;
    assertKind(hasErrors, "Chain");
    assertKind(hasErrors.rest, "Chain");
    assertKind(hasErrors.rest.rest, "Chain");
    expect(hasErrors.rest.rest.rest).toEqual({
      kind: "Step",
      step: { kind: "Root" },
    });
  });

  it("rejects piping a value directly into self", () => {
    workflowBuilder()
      .workflow(({ self }) =>
        // @ts-expect-error — verify outputs {verified: boolean} but self expects never
        pipe(constant({ artifact: "test" }), verify, self),
      );
  });
});

// -----------------------------------------------------------------------
// Mutual recursion via stepRef
// -----------------------------------------------------------------------

describe("mutual recursion", () => {
  it("stepRef enables cross-references between steps", () => {
    const cfg = workflowBuilder()
      .registerSteps(({ stepRef }) => ({
        A: pipe(verify, stepRef("B")),
        B: pipe(verify, stepRef("A")),
      }))
      .workflow(({ steps }) =>
        pipe(constant({ artifact: "test" }), steps.A),
      );

    expect(cfg.steps).toHaveProperty("A");
    expect(cfg.steps).toHaveProperty("B");
    // A body: pipe(verify, stepRef("B")) → Chain(verify, stepRef("B"))
    const aBody = cfg.steps!.A;
    assertKind(aBody, "Chain");
    expect(aBody.rest).toEqual({
      kind: "Step",
      step: { kind: "Named", name: "B" },
    });
    // B body: pipe(verify, stepRef("A")) → Chain(verify, stepRef("A"))
    const bBody = cfg.steps!.B;
    assertKind(bBody, "Chain");
    expect(bBody.rest).toEqual({
      kind: "Step",
      step: { kind: "Named", name: "A" },
    });
  });

  it("callback form provides typed access to previously registered steps", () => {
    const cfg = workflowBuilder()
      .registerSteps({ Setup: setup })
      .registerSteps(({ steps }) => ({
        Pipeline: pipe(steps.Setup, build),
      }))
      .workflow(({ steps }) =>
        pipe(constant({ project: "test" }), steps.Pipeline),
      );

    expect(cfg.steps).toHaveProperty("Setup");
    expect(cfg.steps).toHaveProperty("Pipeline");
    // Pipeline body: pipe(steps.Setup, build) → Chain(steps.Setup, build)
    const pipelineBody = cfg.steps!.Pipeline;
    assertKind(pipelineBody, "Chain");
    expect(pipelineBody.first).toEqual({
      kind: "Step",
      step: { kind: "Named", name: "Setup" },
    });
  });

  it("rejects invalid step references at compile time", () => {
    workflowBuilder()
      .registerSteps(({ stepRef }) => ({
        A: pipe(verify, stepRef("Bt")),
        B: pipe(verify, stepRef("A")),
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
    const cfg = workflowBuilder()
      .registerSteps(({ stepRef }) => ({
        TypeCheck: pipe(
          typeCheck,
          classifyErrors,
          branch({
            HasErrors: stepRef("FixAll"),
            Clean: drop<void>(),
          }),
        ),
        FixAll: pipe(
          forEach(fix),
          stepRef("TypeCheck"),
        ),
      }))
      .workflow(({ steps }) =>
        pipe(
          constant({ project: "my-app" }),
          setup,
          listFiles,
          forEach(migrate),
          steps.TypeCheck,
        ),
      );

    expect(cfg.steps).toHaveProperty("TypeCheck");
    expect(cfg.steps).toHaveProperty("FixAll");
    expect(cfg.workflow.kind).toBe("Chain");
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
    const cfg = workflowBuilder()
      // ── Batch 1 (object form): standalone steps ──
      .registerSteps({
        Setup: setup,
      })
      // ── Batch 2 (callback form): cross-batch refs + mutual recursion ──
      .registerSteps(({ steps, stepRef }) => ({
        // MigrateAll chains through the previously registered Setup step
        MigrateAll: pipe(
          steps.Setup,
          listFiles,
          forEach(migrate),
          stepRef("FixCycle"), // jump to the fix cycle defined below
        ),
        // FixCycle: type-check, classify errors, fix or finish
        FixCycle: loop<void>((recur, done) =>
          pipe(typeCheck, classifyErrors).branch({
            HasErrors: pipe(forEach(fix).drop(), recur),
            Clean: done,
          }),
        ),
      }))
      // ── Workflow: orchestrate with self-restart on persistent errors ──
      .workflow(({ steps, self }) =>
        pipe(
          constant({ project: "my-app" }),
          steps.MigrateAll,
          classifyErrors,
        ).branch({
          HasErrors: pipe(drop<TypeError[]>(), self),  // restart the entire workflow
          Clean: pipe(drop<void>(), constant({ migrated: true })),
        }),
      );

    // ── Verify structure ──

    // All three steps registered
    expect(cfg.steps).toHaveProperty("Setup");
    expect(cfg.steps).toHaveProperty("MigrateAll");
    expect(cfg.steps).toHaveProperty("FixCycle");

    // MigrateAll: pipe(steps.Setup, listFiles, forEach(migrate), stepRef("FixCycle"))
    // → Chain(steps.Setup, Chain(listFiles, Chain(forEach(migrate), stepRef("FixCycle"))))
    const migrateAll = cfg.steps!.MigrateAll;
    assertKind(migrateAll, "Chain");
    expect(migrateAll.first).toEqual({
      kind: "Step",
      step: { kind: "Named", name: "Setup" },
    });

    // MigrateAll ends with a reference to FixCycle (intra-batch via `stepRef`)
    assertKind(migrateAll.rest, "Chain");
    assertKind(migrateAll.rest.rest, "Chain");
    expect(migrateAll.rest.rest.rest).toEqual({
      kind: "Step",
      step: { kind: "Named", name: "FixCycle" },
    });

    // Workflow: pipe(constant, steps.MigrateAll, classifyErrors).branch({...})
    // → Chain(Chain(constant, Chain(MigrateAll, classifyErrors)), Branch({...}))
    assertKind(cfg.workflow, "Chain");
    assertKind(cfg.workflow.rest, "Branch");
    // HasErrors: auto-unwrap + pipe(drop, self)
    // → Chain(ExtractField("value"), Chain(drop, self))
    const hasErrors = cfg.workflow.rest.cases.HasErrors;
    assertKind(hasErrors, "Chain");
    assertKind(hasErrors.rest, "Chain");
    expect(hasErrors.rest.rest).toEqual({
      kind: "Step",
      step: { kind: "Root" },
    });
  });
});
