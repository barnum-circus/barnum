import { describe, it, expect, beforeEach } from "vitest";
import {
  type ExtractInput,
  type ExtractOutput,
  type Result,
  type TypedAction,
  pipe,
  tryCatch,
  race,
  sleep,
  withTimeout,
  resetEffectIdCounter,
} from "../src/ast.js";
import {
  constant,
  drop,
  getField,
  identity,
} from "../src/builtins/index.js";
import { runPipeline } from "../src/run.js";
import { setup, build, verify } from "./handlers.js";

// ---------------------------------------------------------------------------
// Type assertion helpers (compile-time only)
// ---------------------------------------------------------------------------

type IsExact<T, U> = [T] extends [U] ? ([U] extends [T] ? true : false) : false;

// eslint-disable-next-line @typescript-eslint/no-unused-vars
function assertExact<_T extends true>(): void {}

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
    assertExact<IsExact<ExtractInput<typeof action>, { project: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { artifact: string }>>();
  });

  it("throwError token is TypedAction<TError, never>", () => {
    tryCatch(
      (throwError) => {
        assertExact<IsExact<typeof throwError, TypedAction<string, never>>>();
        return identity();
      },
      identity(),
    );
  });

  it("recovery input type matches throwError payload type", () => {
    const action = tryCatch(
      (_throwError: TypedAction<{ code: number; msg: string }, never>) =>
        pipe(drop, constant("ok")),
      getField<{ code: number; msg: string }, "msg">("msg"),
    );
    assertExact<IsExact<ExtractOutput<typeof action>, string>>();
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
        return tryCatch(
          (throwInner) => {
            assertExact<
              IsExact<typeof throwInner, TypedAction<{ artifact: string }, never>>
            >();
            return pipe(drop, constant({ verified: true }));
          },
          verify,
        );
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
    assertExact<IsExact<ExtractInput<typeof action>, { artifact: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { verified: boolean }>>();
  });

  it("race produces Chain(Tag(Continue), Handle(...)) AST", () => {
    const action = race(verify, verify);
    expect(action.kind).toBe("Chain");
  });

  it("sleep: any → void (like drop)", () => {
    const action = sleep(1000);
    assertExact<IsExact<ExtractInput<typeof action>, any>>();
    assertExact<IsExact<ExtractOutput<typeof action>, void>>();
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
    assertExact<IsExact<ExtractInput<typeof action>, { artifact: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Result<{ verified: boolean }, void>>>();
  });

  it("withTimeout produces Chain(Tag(Continue), Handle(...)) AST", () => {
    const action = withTimeout(constant(1000), verify);
    expect(action.kind).toBe("Chain");
  });

  it("withTimeout with any-input body", () => {
    const action = withTimeout(constant(3000), constant("result"));
    assertExact<IsExact<ExtractInput<typeof action>, any>>();
    assertExact<IsExact<ExtractOutput<typeof action>, Result<string, void>>>();
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
      tryCatch(
        (_throwError) => constant("success"),
        constant("recovery"),
      ),
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
    const result = await runPipeline(
      race(constant("a"), constant("b")),
    );
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
