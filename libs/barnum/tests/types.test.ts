import { describe, it, expect } from "vitest";
import { call, sequence } from "../src/core.js";
import setup from "./handlers/setup.js";
import process_ from "./handlers/process.js";
import check from "./handlers/check.js";
import finalize from "./handlers/finalize.js";

describe("sequence type safety", () => {
  it("accepts a valid two-step sequence", () => {
    // setup: SetupInput -> SetupOutput
    // process_: ProcessInput (= SetupOutput) -> ProcessOutput
    const workflow = sequence(call(setup), call(process_));
    expect(workflow.kind).toBe("Sequence");
  });

  it("rejects mismatched sequence types", () => {
    // check: CheckInput -> CheckOutput
    // setup: SetupInput -> SetupOutput
    // CheckOutput ({ valid: boolean }) does not match SetupInput ({ project: string })
    // @ts-expect-error — type mismatch between check's output and setup's input
    sequence(call(check), call(setup));
  });

  it("chains three steps correctly", () => {
    // setup -> process_ -> check (SetupInput -> SetupOutput -> ProcessOutput -> CheckOutput)
    // But wait: process_ expects ProcessInput = { initialized: boolean; project: string }
    // and setup outputs SetupOutput = { initialized: boolean; project: string }
    // Then check expects CheckInput = { result: string }
    // and process_ outputs ProcessOutput = { result: string }
    const workflow = sequence(call(setup), call(process_), call(check));
    expect(workflow.kind).toBe("Sequence");
  });

  it("rejects unrelated types in sequence", () => {
    // finalize: FinalizeInput -> FinalizeOutput
    // setup: SetupInput -> SetupOutput
    // FinalizeOutput ({ done: true }) does not match SetupInput ({ project: string })
    // @ts-expect-error — type mismatch between finalize's output and setup's input
    sequence(call(finalize), call(setup));
  });
});
