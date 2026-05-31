import { beforeEach, describe, expect, it } from "vitest";
import {
  type Result,
  type TypedAction,
  pipe,
  resetEffectIdCounter,
} from "../src/ast.js";
import { constant, identity } from "../src/builtins/index.js";
import { Result as R } from "../src/result.js";
import { withRetry } from "../src/retry.js";
import { runPipeline } from "../src/run.js";
import { assertIO } from "./type-utils.js";

// ---------------------------------------------------------------------------
// Type tests
// ---------------------------------------------------------------------------

describe("withRetry type tests", () => {
  it("input and output types match the action", () => {
    const action = identity() as TypedAction<string, Result<number, string>>;
    const retried = withRetry(action, 3);
    assertIO<typeof retried, string, Result<number, string>>();
  });

  it("works with pipe", () => {
    const action = identity() as TypedAction<number, Result<string, boolean>>;
    const pipeline = pipe(constant(42), withRetry(action, 2));
    assertIO<typeof pipeline, any, Result<string, boolean>>();
  });
});

// ---------------------------------------------------------------------------
// Execution tests
// ---------------------------------------------------------------------------

describe("withRetry execution", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("returns Ok immediately on first success", async () => {
    const result = await runPipeline(
      pipe(constant("input"), withRetry(constant("ok").ok(), 3)),
    );
    expect(result).toEqual({ kind: "Result.Ok", value: "ok" });
  });

  it("always-failing action exhausts all attempts", async () => {
    const result = await runPipeline(
      pipe(constant("input"), withRetry(constant("fail").err(), 3)),
    );
    expect(result).toEqual({ kind: "Result.Err", value: "fail" });
  });

  it("returns final Err after all attempts exhausted", async () => {
    const result = await runPipeline(
      pipe(
        constant("input"),
        withRetry(pipe(constant("error"), R.err<number, string>()), 5),
      ),
    );
    expect(result).toEqual({ kind: "Result.Err", value: "error" });
  });

  it("maxAttempts=1 means no retry (single attempt)", async () => {
    const result = await runPipeline(
      pipe(constant("x"), withRetry(constant("fail").err(), 1)),
    );
    expect(result).toEqual({ kind: "Result.Err", value: "fail" });
  });

  it("Ok short-circuits remaining retries", async () => {
    // If action returns Ok, .or() branches never fire
    const result = await runPipeline(
      pipe(constant(42), withRetry(constant(99).ok(), 10)),
    );
    expect(result).toEqual({ kind: "Result.Ok", value: 99 });
  });

  it("preserves pipeline input across retries", async () => {
    // Use an action that passes input through as Ok
    const result = await runPipeline(
      pipe(constant("preserved"), withRetry(R.ok<string, string>(), 3)),
    );
    expect(result).toEqual({ kind: "Result.Ok", value: "preserved" });
  });
});
