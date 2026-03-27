import { describe, it, expect } from "vitest";
import {
  parallel,
  attempt,
  configBuilder,
  loop,
  branch,
  pipe,
  forEach,
} from "../src/ast.js";
import { constant } from "../src/builtins.js";
import {
  setup,
  process,
  check,
  finalize,
  validate,
} from "./handlers.js";

describe("pipe type safety", () => {
  it("accepts a valid two-step pipe", () => {
    const workflow = pipe(setup(), process());
    expect(workflow.kind).toBe("Pipe");
  });

  it("rejects mismatched pipe types", () => {
    // CheckOutput ({ valid: boolean }) does not match SetupInput ({ project: string })
    // @ts-expect-error — type mismatch between check's output and setup's input
    pipe(check(), setup());
  });

  it("chains three steps correctly", () => {
    const workflow = pipe(setup(), process(), check());
    expect(workflow.kind).toBe("Pipe");
  });

  it("rejects unrelated types in pipe", () => {
    // FinalizeOutput ({ done: true }) does not match SetupInput ({ project: string })
    // @ts-expect-error — type mismatch between finalize's output and setup's input
    pipe(finalize(), setup());
  });
});

describe("parallel type safety", () => {
  it("accepts actions with the same input type", () => {
    // Both check handlers take { result: string }
    const workflow = parallel(check(), check());
    expect(workflow.kind).toBe("Parallel");
  });

  it("rejects actions with different input types", () => {
    // setup expects { project: string }, check expects { result: string }
    // @ts-expect-error — input types do not unify
    parallel(setup(), check());
  });
});

describe("branch type safety", () => {
  it("accepts cases with the same input and output types", () => {
    const workflow = branch({
      Yes: finalize(),
      No: finalize(),
    });
    expect(workflow.kind).toBe("Branch");
  });

  it("rejects branch output flowing into incompatible step", () => {
    // branch outputs { done: true }, but setup expects { project: string }
    // @ts-expect-error — branch output doesn't satisfy next step's input
    pipe(branch({ A: finalize(), B: finalize() }), setup());
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

  it("chains in pipe with result-aware consumer", () => {
    // process_ outputs { result: string }, attempt(check) expects { result: string }
    const workflow = pipe(process(), attempt(check()));
    expect(workflow.kind).toBe("Pipe");
  });
});

describe("forEach type safety", () => {
  it("maps input/output to arrays", () => {
    // forEach(check) takes { result: string }[] and produces { valid: boolean }[]
    const workflow = forEach(check());
    expect(workflow.kind).toBe("ForEach");
  });
});

describe("named step type safety", () => {
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
});
