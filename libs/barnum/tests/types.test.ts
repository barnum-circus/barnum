import { describe, it, expect } from "vitest";
import { all, call, configBuilder, sequence } from "../src/core.js";
import setup from "./handlers/setup.js";
import process_ from "./handlers/process.js";
import check from "./handlers/check.js";
import finalize from "./handlers/finalize.js";

describe("sequence type safety", () => {
  it("accepts a valid two-step sequence", () => {
    const workflow = sequence(call(setup), call(process_));
    expect(workflow.kind).toBe("Sequence");
  });

  it("rejects mismatched sequence types", () => {
    // CheckOutput ({ valid: boolean }) does not match SetupInput ({ project: string })
    // @ts-expect-error — type mismatch between check's output and setup's input
    sequence(call(check), call(setup));
  });

  it("chains three steps correctly", () => {
    const workflow = sequence(call(setup), call(process_), call(check));
    expect(workflow.kind).toBe("Sequence");
  });

  it("rejects unrelated types in sequence", () => {
    // FinalizeOutput ({ done: true }) does not match SetupInput ({ project: string })
    // @ts-expect-error — type mismatch between finalize's output and setup's input
    sequence(call(finalize), call(setup));
  });
});

describe("named step type safety", () => {
  it("allows referencing registered steps", () => {
    const cfg = configBuilder()
      .registerSteps({ Finalize: call(finalize) })
      .workflow((steps) => steps.Finalize);
    expect(cfg.workflow.kind).toBe("Step");
  });

  it("rejects references to unregistered steps", () => {
    configBuilder()
      .registerSteps({ Finalize: call(finalize) })
      .workflow((steps) => {
        // @ts-expect-error — "Nonexistent" was never registered
        return steps.Nonexistent;
      });
  });

  it("supports multiple registerSteps calls", () => {
    const cfg = configBuilder()
      .registerSteps({ Finalize: call(finalize) })
      .registerSteps({ Restart: call(setup) })
      .workflow((steps) => all(steps.Finalize, steps.Restart));
    expect(cfg.steps).toHaveProperty("Finalize");
    expect(cfg.steps).toHaveProperty("Restart");
  });
});
