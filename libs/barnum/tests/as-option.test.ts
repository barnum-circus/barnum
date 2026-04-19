import { describe, it, expect } from "vitest";
import {
  type ExtractInput,
  type ExtractOutput,
  type Option,
  pipe,
} from "../src/ast.js";
import { constant } from "../src/builtins/index.js";
import { asOption } from "../src/builtins/index.js";
import { runPipeline } from "../src/run.js";

// ---------------------------------------------------------------------------
// Type assertion helpers (compile-time only)
// ---------------------------------------------------------------------------

type IsExact<T, U> = [T] extends [U] ? ([U] extends [T] ? true : false) : false;

// eslint-disable-next-line @typescript-eslint/no-unused-vars
function assertExact<_T extends true>(): void {}

// ---------------------------------------------------------------------------
// Type tests
// ---------------------------------------------------------------------------

describe("AsOption type tests", () => {
  it("asOption() standalone: boolean → Option<void>", () => {
    const action = asOption();
    assertExact<IsExact<ExtractInput<typeof action>, boolean>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Option<void>>>();
  });

  it(".asOption() postfix: infers from preceding output", () => {
    const action = constant(true).asOption();
    assertExact<IsExact<ExtractOutput<typeof action>, Option<void>>>();
  });

  it("asOption() composes in pipe", () => {
    const action = pipe(constant(true), asOption());
    assertExact<IsExact<ExtractOutput<typeof action>, Option<void>>>();
  });
});

// ---------------------------------------------------------------------------
// Execution tests
// ---------------------------------------------------------------------------

describe("AsOption execution", () => {
  it("true → Some(void)", async () => {
    const result = await runPipeline(constant(true).asOption());
    expect(result).toEqual({ kind: "Option.Some", value: null });
  });

  it("false → None", async () => {
    const result = await runPipeline(constant(false).asOption());
    expect(result).toEqual({ kind: "Option.None", value: null });
  });

  it("standalone asOption() in pipe: true → Some", async () => {
    const result = await runPipeline(pipe(constant(true), asOption()));
    expect(result).toEqual({ kind: "Option.Some", value: null });
  });

  it("standalone asOption() in pipe: false → None", async () => {
    const result = await runPipeline(pipe(constant(false), asOption()));
    expect(result).toEqual({ kind: "Option.None", value: null });
  });

  it("asOption + branch dispatches correctly", async () => {
    const result = await runPipeline(
      constant(true).asOption().branch({
        Some: constant("was true"),
        None: constant("was false"),
      }),
    );
    expect(result).toBe("was true");
  });

  it("asOption false + branch dispatches to None", async () => {
    const result = await runPipeline(
      constant(false).asOption().branch({
        Some: constant("was true"),
        None: constant("was false"),
      }),
    );
    expect(result).toBe("was false");
  });
});
