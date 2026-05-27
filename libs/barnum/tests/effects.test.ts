import { beforeEach, describe, expect, it } from "vitest";
import {
  type Result,
  type TypedAction,
  pipe,
  race,
  resetEffectIdCounter,
  sleep,
  tryCatch,
  withTimeout,
} from "../src/ast.js";
import { constant, drop, getField, identity } from "../src/builtins/index.js";
import { runPipeline } from "../src/run.js";
import { build, setup, verify } from "./handlers.js";
import { type IsExact, assertExact, assertIO } from "./type-utils.js";

// ---------------------------------------------------------------------------
// tryCatch type tests
// ---------------------------------------------------------------------------

describe("tryCatch type tests", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("tryCatch: input from body, output matches body and recovery", () => {
    const action = tryCatch(
      (_throwError) => pipe(setup, build),
      pipe(drop, constant({ artifact: "fallback" })),
    );
    assertIO<typeof action, { project: string }, { artifact: string }>();
  });

  it("throwError token is TypedAction<TError, never>", () => {
    tryCatch((throwError) => {
      assertExact<IsExact<typeof throwError, TypedAction<string, never>>>();
      return identity();
    }, identity());
  });

  it("recovery input type matches throwError payload type", () => {
    const action = tryCatch(
      (_throwError: TypedAction<{ code: number; msg: string }, never>) =>
        pipe(drop, constant("ok")),
      getField<{ code: number; msg: string }, "msg">("msg"),
    );
    assertIO<typeof action, any, string>();
  });

  it("nested tryCatch: each throwError has independent TError", () => {
    tryCatch(
      (throwOuter) => {
        assertExact<
          IsExact<
            typeof throwOuter,
            TypedAction<{ initialized: boolean; project: string }, never>
          >
        >();
        return tryCatch((throwInner) => {
          assertExact<
            IsExact<typeof throwInner, TypedAction<{ artifact: string }, never>>
          >();
          return pipe(drop, constant({ verified: true }));
        }, verify);
      },
      pipe(build, verify),
    );
  });

  it("tryCatch produces Chain(Tag(Continue), Handle(...)) AST", () => {
    const action = tryCatch(
      (_throwError) => pipe(drop, constant("ok")),
      identity(),
    );
    expect(action.kind).toBe("Chain");
  });
});

// ---------------------------------------------------------------------------
// race type tests
// ---------------------------------------------------------------------------

describe("race type tests", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("race: all branches same input/output, result matches", () => {
    const action = race(verify, verify);
    assertIO<typeof action, { artifact: string }, { verified: boolean }>();
  });

  it("race produces Chain(Tag(Continue), Handle(...)) AST", () => {
    const action = race(verify, verify);
    expect(action.kind).toBe("Chain");
  });

  it("sleep: any → null", () => {
    const action = sleep(1000);
    assertIO<typeof action, any, null>();
  });

  it("sleep produces Invoke AST", () => {
    const action = sleep(1000);
    expect(action.kind).toBe("Invoke");
  });
});

// ---------------------------------------------------------------------------
// withTimeout type tests
// ---------------------------------------------------------------------------

describe("withTimeout type tests", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("withTimeout: preserves input, wraps output in Result<TOut, void>", () => {
    const action = withTimeout(constant(5000), verify);
    assertIO<
      typeof action,
      { artifact: string },
      Result<{ verified: boolean }, void>
    >();
  });

  it("withTimeout produces Chain(Tag(Continue), Handle(...)) AST", () => {
    const action = withTimeout(constant(1000), verify);
    expect(action.kind).toBe("Chain");
  });

  it("withTimeout with any-input body", () => {
    const action = withTimeout(constant(3000), constant("result"));
    assertIO<typeof action, any, Result<string, void>>();
  });
});

// ---------------------------------------------------------------------------
// Result.unwrapOr with throw tokens
// ---------------------------------------------------------------------------

describe("Result.unwrapOr with throw tokens", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("rejects .unwrapOr() on non-Result output", () => {
    // @ts-expect-error — unwrapOr requires Option or Result output
    verify.unwrapOr(drop);
  });
});

// ---------------------------------------------------------------------------
// Execution tests
// ---------------------------------------------------------------------------

describe("tryCatch execution", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("body succeeds, returns body result", async () => {
    const result = await runPipeline(
      tryCatch((_throwError) => constant("success"), constant("recovery")),
    );
    expect(result).toBe("success");
  });

  it("body throws, recovery runs with error value", async () => {
    const result = await runPipeline(
      tryCatch(
        (throwError) => pipe(constant("error-payload"), throwError),
        identity(),
      ),
    );
    expect(result).toBe("error-payload");
  });

  it("nested tryCatch with independent errors", async () => {
    const result = await runPipeline(
      tryCatch(
        (_outerThrow) =>
          tryCatch(
            (innerThrow) => pipe(constant("inner-error"), innerThrow),
            identity(),
          ),
        constant("outer-recovery"),
      ),
    );
    // Inner throw is caught by inner recovery, outer tryCatch sees success
    expect(result).toBe("inner-error");
  });
});

describe("race execution", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("race returns first completed result", async () => {
    // Both branches are constant (instant), but race should still return one of them
    const result = await runPipeline(race(constant("a"), constant("b")));
    // Either "a" or "b" — both are valid. In practice, first branch wins.
    expect(["a", "b"]).toContain(result);
  });
});

describe("sleep execution", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("sleep returns null (void)", async () => {
    const result = await runPipeline(sleep(1));
    expect(result).toBeNull();
  });
});
