import { describe, it, expect } from "vitest";
import {
  all,
  attempt,
  configBuilder,
  loop,
  matchCases,
  sequence,
  traverse,
} from "../src/ast.js";
import { constant } from "../src/builtins.js";
import setup from "./handlers/setup.js";
import process_ from "./handlers/process.js";
import check from "./handlers/check.js";
import finalize from "./handlers/finalize.js";
import validate from "./handlers/validate.js";

describe("sequence type safety", () => {
  it("accepts a valid two-step sequence", () => {
    const workflow = sequence(setup(), process_());
    expect(workflow.kind).toBe("Sequence");
  });

  it("rejects mismatched sequence types", () => {
    // CheckOutput ({ valid: boolean }) does not match SetupInput ({ project: string })
    // @ts-expect-error — type mismatch between check's output and setup's input
    sequence(check(), setup());
  });

  it("chains three steps correctly", () => {
    const workflow = sequence(setup(), process_(), check());
    expect(workflow.kind).toBe("Sequence");
  });

  it("rejects unrelated types in sequence", () => {
    // FinalizeOutput ({ done: true }) does not match SetupInput ({ project: string })
    // @ts-expect-error — type mismatch between finalize's output and setup's input
    sequence(finalize(), setup());
  });
});

describe("all type safety", () => {
  it("accepts actions with the same input type", () => {
    // Both check handlers take { result: string }
    const workflow = all(check(), check());
    expect(workflow.kind).toBe("All");
  });

  it("rejects actions with different input types", () => {
    // setup expects { project: string }, check expects { result: string }
    // @ts-expect-error — input types do not unify
    all(setup(), check());
  });
});

describe("matchCases type safety", () => {
  it("accepts cases with the same input and output types", () => {
    const workflow = matchCases({
      Yes: finalize(),
      No: finalize(),
    });
    expect(workflow.kind).toBe("Match");
  });

  it("rejects match output flowing into incompatible step", () => {
    // matchCases outputs { done: true }, but setup expects { project: string }
    // @ts-expect-error — match output doesn't satisfy next step's input
    sequence(matchCases({ A: finalize(), B: finalize() }), setup());
  });
});

describe("loop type safety", () => {
  it("accepts body returning LoopResult", () => {
    // validate: { valid: boolean } → LoopResult<{ valid: boolean }, { done: true }>
    // loop infers: TypedAction<{ valid: boolean }, { done: true }>
    const workflow = loop(validate());
    expect(workflow.kind).toBe("Loop");
  });

  it("rejects body not returning LoopResult", () => {
    // check: { result: string } → { valid: boolean } — not a LoopResult
    // @ts-expect-error — loop body must return LoopResult<In, Out>
    loop(check());
  });
});

describe("attempt type safety", () => {
  it("wraps output in AttemptResult", () => {
    // attempt(check) takes { result: string } and produces AttemptResult<{ valid: boolean }>
    const wrapped = attempt(check());
    expect(wrapped.kind).toBe("Attempt");
  });

  it("chains in sequence with result-aware consumer", () => {
    // process_ outputs { result: string }, attempt(check) expects { result: string }
    const workflow = sequence(process_(), attempt(check()));
    expect(workflow.kind).toBe("Sequence");
  });
});

describe("traverse type safety", () => {
  it("maps input/output to arrays", () => {
    // traverse(check) takes { result: string }[] and produces { valid: boolean }[]
    const workflow = traverse(check());
    expect(workflow.kind).toBe("Traverse");
  });
});

describe("named step type safety", () => {
  it("allows referencing registered steps", () => {
    const cfg = configBuilder()
      .registerSteps({ Finalize: finalize() })
      .workflow((steps) => sequence(constant({ valid: true }), steps.Finalize));
    expect(cfg.workflow.kind).toBe("Sequence");
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
        sequence(constant({ valid: true }), all(steps.Finalize, steps.Revalidate)),
      );
    expect(cfg.steps).toHaveProperty("Finalize");
    expect(cfg.steps).toHaveProperty("Revalidate");
  });
});
