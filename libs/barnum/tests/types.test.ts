import { describe, it, expect } from "vitest";
import {
  all,
  attempt,
  call,
  configBuilder,
  loop,
  matchCases,
  sequence,
  traverse,
} from "../src/core.js";
import setup from "./handlers/setup.js";
import process_ from "./handlers/process.js";
import check from "./handlers/check.js";
import finalize from "./handlers/finalize.js";
import validate from "./handlers/validate.js";

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

describe("all type safety", () => {
  it("accepts actions with the same input type", () => {
    // Both check handlers take { result: string }
    const workflow = all(call(check), call(check));
    expect(workflow.kind).toBe("All");
  });

  it("rejects actions with different input types", () => {
    // setup expects { project: string }, check expects { result: string }
    // @ts-expect-error — input types do not unify
    all(call(setup), call(check));
  });
});

describe("matchCases type safety", () => {
  it("accepts cases with the same input and output types", () => {
    const workflow = matchCases({
      yes: call(finalize),
      no: call(finalize),
    });
    expect(workflow.kind).toBe("Match");
  });

  it("rejects match output flowing into incompatible step", () => {
    // matchCases outputs { done: true }, but setup expects { project: string }
    // @ts-expect-error — match output doesn't satisfy next step's input
    sequence(matchCases({ a: call(finalize), b: call(finalize) }), call(setup));
  });
});

describe("loop type safety", () => {
  it("accepts body returning LoopResult", () => {
    // validate: { valid: boolean } → LoopResult<{ valid: boolean }, { done: true }>
    // loop infers: TypedAction<{ valid: boolean }, { done: true }>
    const workflow = loop(call(validate));
    expect(workflow.kind).toBe("Loop");
  });

  it("rejects body not returning LoopResult", () => {
    // check: { result: string } → { valid: boolean } — not a LoopResult
    // @ts-expect-error — loop body must return LoopResult<In, Out>
    loop(call(check));
  });
});

describe("attempt type safety", () => {
  it("wraps output in AttemptResult", () => {
    // attempt(check) takes { result: string } and produces AttemptResult<{ valid: boolean }>
    const wrapped = attempt(call(check));
    expect(wrapped.kind).toBe("Attempt");
  });

  it("chains in sequence with result-aware consumer", () => {
    // process_ outputs { result: string }, attempt(check) expects { result: string }
    const workflow = sequence(call(process_), attempt(call(check)));
    expect(workflow.kind).toBe("Sequence");
  });
});

describe("traverse type safety", () => {
  it("maps input/output to arrays", () => {
    // traverse(check) takes { result: string }[] and produces { valid: boolean }[]
    const workflow = traverse(call(check));
    expect(workflow.kind).toBe("Traverse");
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
      .registerSteps({ Revalidate: call(validate) })
      .workflow((steps) => all(steps.Finalize, steps.Revalidate));
    expect(cfg.steps).toHaveProperty("Finalize");
    expect(cfg.steps).toHaveProperty("Revalidate");
  });
});
