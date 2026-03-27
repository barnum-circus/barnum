import { describe, it, expect } from "vitest";
import { call, sequence, typescript } from "../src/core.js";

type Input = { name: string };
type Middle = { greeting: string };
type Output = { farewell: string };

describe("sequence type safety", () => {
  const greet = typescript<Input, Middle>("./h.ts", "greet");
  const farewell = typescript<Middle, Output>("./h.ts", "farewell");
  const unrelated = typescript<{ x: number }, { y: boolean }>(
    "./h.ts",
    "other",
  );

  it("accepts a valid two-step sequence", () => {
    const workflow = sequence(call(greet), call(farewell));
    expect(workflow.kind).toBe("Sequence");
  });

  it("rejects mismatched sequence types", () => {
    // Output of farewell ({ farewell: string }) does not match input of greet ({ name: string })
    // @ts-expect-error — type mismatch between farewell's output and greet's input
    sequence(call(farewell), call(greet));
  });

  it("rejects unrelated types in sequence", () => {
    // Output of greet ({ greeting: string }) does not match input of unrelated ({ x: number })
    // @ts-expect-error — type mismatch between greet's output and unrelated's input
    sequence(call(greet), call(unrelated));
  });

  it("chains three steps correctly", () => {
    const step1 = typescript<Input, Middle>("./h.ts", "a");
    const step2 = typescript<Middle, Output>("./h.ts", "b");
    const step3 = typescript<Output, { done: true }>("./h.ts", "c");

    const workflow = sequence(call(step1), call(step2), call(step3));
    expect(workflow.kind).toBe("Sequence");
  });
});
