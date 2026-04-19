import { describe, it, expect } from "vitest";
import { z } from "zod";
import { createHandler, createHandlerWithConfig } from "../src/handler.js";
import {
  type ExtractInput,
  type ExtractOutput,
  type LoopResult,
  pipe,
  config,
} from "../src/ast.js";
import {
  setup,
  build,
  verify,
  deploy,
  healthCheck,
  listFiles,
  migrate,
  typeCheck,
  fix,
} from "./handlers.js";

// ---------------------------------------------------------------------------
// Type assertion helpers (compile-time only)
// ---------------------------------------------------------------------------

type IsExact<T, U> = [T] extends [U] ? ([U] extends [T] ? true : false) : false;

// eslint-disable-next-line @typescript-eslint/no-unused-vars
function assertExact<_T extends true>(): void {}

// ---------------------------------------------------------------------------
// Handler type tests — test fixture handlers
// ---------------------------------------------------------------------------

describe("handler types", () => {
  it("setup: { project: string } -> { initialized: boolean, project: string }", () => {
    const action = setup;
    assertExact<IsExact<ExtractInput<typeof action>, { project: string }>>();
    assertExact<
      IsExact<
        ExtractOutput<typeof action>,
        { initialized: boolean; project: string }
      >
    >();
    expect(action.kind).toBe("Invoke");
  });

  it("build: { initialized: boolean, project: string } -> { artifact: string }", () => {
    const action = build;
    assertExact<
      IsExact<
        ExtractInput<typeof action>,
        { initialized: boolean; project: string }
      >
    >();
    assertExact<IsExact<ExtractOutput<typeof action>, { artifact: string }>>();
    expect(action.kind).toBe("Invoke");
  });

  it("verify: { artifact: string } -> { verified: boolean }", () => {
    const action = verify;
    assertExact<IsExact<ExtractInput<typeof action>, { artifact: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { verified: boolean }>>();
    expect(action.kind).toBe("Invoke");
  });

  it("deploy: { verified: boolean } -> { deployed: boolean }", () => {
    const action = deploy;
    assertExact<IsExact<ExtractInput<typeof action>, { verified: boolean }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { deployed: boolean }>>();
    expect(action.kind).toBe("Invoke");
  });

  it("healthCheck: { deployed: boolean } -> LoopResult<{ deployed: boolean }, { stable: true }>", () => {
    const action = healthCheck;
    assertExact<IsExact<ExtractInput<typeof action>, { deployed: boolean }>>();
    assertExact<
      IsExact<
        ExtractOutput<typeof action>,
        LoopResult<{ deployed: boolean }, { stable: true }>
      >
    >();
    expect(action.kind).toBe("Invoke");
  });

  it("listFiles: { initialized: boolean, project: string } -> { file: string }[]", () => {
    const action = listFiles;
    assertExact<
      IsExact<
        ExtractInput<typeof action>,
        { initialized: boolean; project: string }
      >
    >();
    assertExact<IsExact<ExtractOutput<typeof action>, { file: string }[]>>();
    expect(action.kind).toBe("Invoke");
  });

  it("migrate: { file: string } -> { file: string, migrated: boolean }", () => {
    const action = migrate;
    assertExact<IsExact<ExtractInput<typeof action>, { file: string }>>();
    assertExact<
      IsExact<ExtractOutput<typeof action>, { file: string; migrated: boolean }>
    >();
    expect(action.kind).toBe("Invoke");
  });

  it("typeCheck: never -> TypeError[]", () => {
    const action = typeCheck;
    assertExact<IsExact<ExtractInput<typeof action>, never>>();
    assertExact<
      IsExact<ExtractOutput<typeof action>, { file: string; message: string }[]>
    >();
    expect(action.kind).toBe("Invoke");
  });

  it("fix: { file: string, message: string } -> { file: string, fixed: boolean }", () => {
    const action = fix;
    assertExact<
      IsExact<ExtractInput<typeof action>, { file: string; message: string }>
    >();
    assertExact<
      IsExact<ExtractOutput<typeof action>, { file: string; fixed: boolean }>
    >();
    expect(action.kind).toBe("Invoke");
  });
});

// ---------------------------------------------------------------------------
// createHandler — optional validators
// ---------------------------------------------------------------------------

describe("createHandler optional types", () => {
  // --- inputValidator infers TValue ---

  it("inputValidator infers TValue", () => {
    const handler = createHandler(
      {
        inputValidator: z.object({ name: z.string() }),
        handle: async ({ value }) => value.name.length,
      },
      "handler",
    );
    assertExact<IsExact<ExtractInput<typeof handler>, { name: string }>>();
    assertExact<IsExact<ExtractOutput<typeof handler>, number>>();
  });

  it("inputValidator + outputValidator infers both", () => {
    const handler = createHandler(
      {
        inputValidator: z.string(),
        outputValidator: z.number(),
        handle: async ({ value }) => value.length,
      },
      "handler",
    );
    assertExact<IsExact<ExtractInput<typeof handler>, string>>();
    assertExact<IsExact<ExtractOutput<typeof handler>, number>>();
  });

  // --- source handler (no inputValidator) ---

  it("source handler: input is void", () => {
    const handler = createHandler(
      {
        handle: async () => "hello",
      },
      "handler",
    );
    assertExact<IsExact<ExtractInput<typeof handler>, void>>();
    assertExact<IsExact<ExtractOutput<typeof handler>, string>>();
  });

  it("source handler with outputValidator", () => {
    const handler = createHandler(
      {
        outputValidator: z.array(z.string()),
        handle: async () => ["a", "b"],
      },
      "handler",
    );
    assertExact<IsExact<ExtractInput<typeof handler>, void>>();
    assertExact<IsExact<ExtractOutput<typeof handler>, string[]>>();
  });

  // --- explicit type params without validators ---

  it("explicit type params: typed input without validator", () => {
    const handler = createHandler<{ id: number }, string>(
      {
        handle: async ({ value }) => String(value.id),
      },
      "handler",
    );
    assertExact<IsExact<ExtractInput<typeof handler>, { id: number }>>();
    assertExact<IsExact<ExtractOutput<typeof handler>, string>>();
  });

  it("explicit type params with outputValidator", () => {
    const handler = createHandler<string, number>(
      {
        outputValidator: z.number(),
        handle: async ({ value }) => value.length,
      },
      "handler",
    );
    assertExact<IsExact<ExtractInput<typeof handler>, string>>();
    assertExact<IsExact<ExtractOutput<typeof handler>, number>>();
  });

  // --- handle must match declared types ---

  it("rejects handle that returns wrong type for explicit TOutput", () => {
    createHandler<string, number>(
      // @ts-expect-error — handle returns string, TOutput is number
      { handle: async ({ value }) => value.toUpperCase() },
      "handler",
    );
  });

  it("rejects handle that uses wrong type for explicit TValue", () => {
    createHandler<string, number>(
      {
        // @ts-expect-error — value is string, not number; .toFixed doesn't exist
        handle: async ({ value }) => value.toFixed(2),
      },
      "handler",
    );
  });

  // --- validators must match declared types ---

  it("rejects inputValidator that contradicts explicit TValue", () => {
    createHandler<string, number>(
      // @ts-expect-error — TValue is string but validator is z.number()
      { inputValidator: z.number(), handle: async ({ value }) => value.length },
      "handler",
    );
  });

  it("rejects outputValidator that contradicts explicit TOutput", () => {
    createHandler<string, number>(
      {
        inputValidator: z.string(),
        // @ts-expect-error — TOutput is number but validator is z.string()
        outputValidator: z.string(),
        handle: async ({ value }) => value.length,
      },
      "handler",
    );
  });

  it("rejects outputValidator that contradicts inferred TOutput", () => {
    createHandler(
      {
        inputValidator: z.string(),
        // @ts-expect-error — handle returns number, outputValidator is z.string()
        outputValidator: z.string(),
        handle: async ({ value }) => value.length,
      },
      "handler",
    );
  });

  it("rejects inputValidator that contradicts handle parameter", () => {
    createHandler(
      {
        inputValidator: z.number(),
        // @ts-expect-error — validator says number, handle destructures string methods
        handle: async ({ value }) => value.toUpperCase(),
      },
      "handler",
    );
  });

  // --- validators must match explicit types (wider rejects, narrower allowed by covariance) ---

  it("rejects inputValidator wider than explicit TValue", () => {
    createHandler<"hello", string>(
      // @ts-expect-error — TValue is "hello" but validator accepts any string
      { inputValidator: z.string(), handle: async ({ value }) => value },
      "handler",
    );
  });

  it("rejects outputValidator wider than explicit TOutput", () => {
    createHandler<string, "ok">(
      {
        inputValidator: z.string(),
        // @ts-expect-error — TOutput is "ok" but validator accepts any string
        outputValidator: z.string(),
        handle: async ({ value: _value }) => "ok" as const,
      },
      "handler",
    );
  });

  it("accepts inputValidator that exactly matches explicit TValue", () => {
    const handler = createHandler<string, number>(
      {
        inputValidator: z.string(),
        handle: async ({ value }) => value.length,
      },
      "handler",
    );
    assertExact<IsExact<ExtractInput<typeof handler>, string>>();
    assertExact<IsExact<ExtractOutput<typeof handler>, number>>();
  });

  it("accepts outputValidator that exactly matches explicit TOutput", () => {
    const handler = createHandler<string, number>(
      {
        inputValidator: z.string(),
        outputValidator: z.number(),
        handle: async ({ value }) => value.length,
      },
      "handler",
    );
    assertExact<IsExact<ExtractInput<typeof handler>, string>>();
    assertExact<IsExact<ExtractOutput<typeof handler>, number>>();
  });

  // --- source handlers in workflows ---

  it("source handler is accepted as config entry point", () => {
    const handler = createHandler(
      {
        handle: async () => "result",
      },
      "handler",
    );
    config(handler);
  });

  // --- pipeline composition ---

  it("validator-typed handlers compose in pipe", () => {
    const toLength = createHandler(
      {
        inputValidator: z.string(),
        handle: async ({ value }) => value.length,
      },
      "toLength",
    );
    const double = createHandler(
      {
        inputValidator: z.number(),
        handle: async ({ value }) => value * 2,
      },
      "double",
    );
    const pipeline = pipe(toLength, double);
    assertExact<IsExact<ExtractInput<typeof pipeline>, string>>();
    assertExact<IsExact<ExtractOutput<typeof pipeline>, number>>();
  });

  it("explicit-typed handlers compose in pipe", () => {
    const toLength = createHandler<string, number>(
      {
        handle: async ({ value }) => value.length,
      },
      "toLength",
    );
    const double = createHandler<number, number>(
      {
        handle: async ({ value }) => value * 2,
      },
      "double",
    );
    const pipeline = pipe(toLength, double);
    assertExact<IsExact<ExtractInput<typeof pipeline>, string>>();
    assertExact<IsExact<ExtractOutput<typeof pipeline>, number>>();
  });

  it("mixed validator + explicit compose in pipe", () => {
    const toLength = createHandler(
      {
        inputValidator: z.string(),
        handle: async ({ value }) => value.length,
      },
      "toLength",
    );
    const double = createHandler<number, number>(
      {
        handle: async ({ value }) => value * 2,
      },
      "double",
    );
    const pipeline = pipe(toLength, double);
    assertExact<IsExact<ExtractInput<typeof pipeline>, string>>();
    assertExact<IsExact<ExtractOutput<typeof pipeline>, number>>();
  });

  it("pipe rejects mismatched adjacent types", () => {
    const toString = createHandler(
      {
        inputValidator: z.string(),
        handle: async ({ value }) => value.toUpperCase(),
      },
      "toString",
    );
    const fromNumber = createHandler(
      {
        inputValidator: z.number(),
        handle: async ({ value }) => value * 2,
      },
      "fromNumber",
    );
    // @ts-expect-error — toString outputs string, fromNumber expects number
    pipe(toString, fromNumber);
  });

  it("source handler composes at pipe start", () => {
    const source = createHandler(
      {
        handle: async () => 42,
      },
      "source",
    );
    const double = createHandler(
      {
        inputValidator: z.number(),
        handle: async ({ value }) => value * 2,
      },
      "double",
    );
    const pipeline = pipe(source, double);
    assertExact<IsExact<ExtractInput<typeof pipeline>, any>>();
    assertExact<IsExact<ExtractOutput<typeof pipeline>, number>>();
  });

  it("postfix .then() works with explicit-typed handler", () => {
    const toLength = createHandler(
      {
        inputValidator: z.string(),
        handle: async ({ value }) => value.length,
      },
      "toLength",
    );
    const double = createHandler<number, number>(
      {
        handle: async ({ value }) => value * 2,
      },
      "double",
    );
    const chained = toLength.then(double);
    assertExact<IsExact<ExtractInput<typeof chained>, string>>();
    assertExact<IsExact<ExtractOutput<typeof chained>, number>>();
  });
});

// ---------------------------------------------------------------------------
// createHandlerWithConfig — optional validators
// ---------------------------------------------------------------------------

describe("createHandlerWithConfig optional types", () => {
  // --- stepConfigValidator optional ---

  it("omitting stepConfigValidator: stepConfig is unknown", () => {
    const factory = createHandlerWithConfig(
      {
        handle: async ({ stepConfig }) => String(stepConfig),
      },
      "handler",
    );
    const action = factory("anything");
    assertExact<IsExact<ExtractInput<typeof action>, void>>();
    assertExact<IsExact<ExtractOutput<typeof action>, string>>();
  });

  it("stepConfigValidator provided: stepConfig is typed", () => {
    const factory = createHandlerWithConfig(
      {
        stepConfigValidator: z.object({ retries: z.number() }),
        handle: async ({ stepConfig }) => stepConfig.retries,
      },
      "handler",
    );
    const action = factory({ retries: 3 });
    assertExact<IsExact<ExtractInput<typeof action>, void>>();
    assertExact<IsExact<ExtractOutput<typeof action>, number>>();
  });

  it("explicit TStepConfig without validator", () => {
    const factory = createHandlerWithConfig<never, string, { retries: number }>(
      {
        handle: async ({ stepConfig }) => String(stepConfig.retries),
      },
      "handler",
    );
    const action = factory({ retries: 3 });
    assertExact<IsExact<ExtractInput<typeof action>, never>>();
    assertExact<IsExact<ExtractOutput<typeof action>, string>>();
  });

  // --- inputValidator optional ---

  it("with inputValidator: input is typed", () => {
    const factory = createHandlerWithConfig(
      {
        inputValidator: z.string(),
        stepConfigValidator: z.object({ retries: z.number() }),
        handle: async ({ value, stepConfig }) =>
          `${value}:${stepConfig.retries}`,
      },
      "handler",
    );
    const action = factory({ retries: 3 });
    assertExact<IsExact<ExtractInput<typeof action>, string>>();
    assertExact<IsExact<ExtractOutput<typeof action>, string>>();
  });

  it("without inputValidator: input defaults to void", () => {
    const factory = createHandlerWithConfig(
      {
        handle: async ({ value, stepConfig: _stepConfig }) => String(value),
      },
      "handler",
    );
    const action = factory("anything");
    assertExact<IsExact<ExtractInput<typeof action>, void>>();
    assertExact<IsExact<ExtractOutput<typeof action>, string>>();
  });

  it("explicit type params without inputValidator", () => {
    const factory = createHandlerWithConfig<
      string,
      number,
      { retries: number }
    >(
      {
        handle: async ({ value, stepConfig }) =>
          value.length + stepConfig.retries,
      },
      "handler",
    );
    const action = factory({ retries: 3 });
    assertExact<IsExact<ExtractInput<typeof action>, string>>();
    assertExact<IsExact<ExtractOutput<typeof action>, number>>();
  });

  // --- all validators present ---

  it("all three validators", () => {
    const factory = createHandlerWithConfig(
      {
        inputValidator: z.string(),
        outputValidator: z.number(),
        stepConfigValidator: z.object({ retries: z.number() }),
        handle: async ({ value, stepConfig }) =>
          value.length + stepConfig.retries,
      },
      "handler",
    );
    const action = factory({ retries: 3 });
    assertExact<IsExact<ExtractInput<typeof action>, string>>();
    assertExact<IsExact<ExtractOutput<typeof action>, number>>();
  });

  // --- type errors when lying ---

  it("rejects wrong stepConfigValidator", () => {
    createHandlerWithConfig<never, string, { retries: number }>(
      {
        // @ts-expect-error — explicit TStepConfig is { retries: number }, validator is z.string()
        stepConfigValidator: z.string(),
        handle: async ({ stepConfig }) => String(stepConfig),
      },
      "handler",
    );
  });

  it("rejects handle that lies about stepConfig shape", () => {
    createHandlerWithConfig(
      {
        stepConfigValidator: z.object({ retries: z.number() }),
        // @ts-expect-error — stepConfig.retries is number, not string method
        handle: async ({ stepConfig }) => stepConfig.retries.toUpperCase(),
      },
      "handler",
    );
  });

  it("rejects outputValidator contradicting handle return", () => {
    createHandlerWithConfig(
      // @ts-expect-error — handle returns number, outputValidator is z.string()
      { outputValidator: z.string(), handle: async ({ stepConfig }) => 42 },
      "handler",
    );
  });

  // --- validators must match explicit types (wider rejects) ---

  it("rejects stepConfigValidator wider than explicit TStepConfig", () => {
    createHandlerWithConfig<never, string, { retries: 3 }>(
      {
        // @ts-expect-error — TStepConfig is { retries: 3 } but validator accepts any { retries: number }
        stepConfigValidator: z.object({ retries: z.number() }),
        handle: async ({ stepConfig }) => String(stepConfig.retries),
      },
      "handler",
    );
  });

  it("accepts all validators exactly matching explicit types", () => {
    const factory = createHandlerWithConfig<
      string,
      number,
      { retries: number }
    >(
      {
        inputValidator: z.string(),
        outputValidator: z.number(),
        stepConfigValidator: z.object({ retries: z.number() }),
        handle: async ({ value, stepConfig }) =>
          value.length + stepConfig.retries,
      },
      "handler",
    );
    const action = factory({ retries: 3 });
    assertExact<IsExact<ExtractInput<typeof action>, string>>();
    assertExact<IsExact<ExtractOutput<typeof action>, number>>();
  });

  // --- pipeline composition ---

  it("withConfig handler composes in pipe", () => {
    const source = createHandler(
      {
        handle: async () => "hello",
      },
      "source",
    );
    const withRetries = createHandlerWithConfig(
      {
        inputValidator: z.string(),
        stepConfigValidator: z.object({ retries: z.number() }),
        handle: async ({ value, stepConfig }) =>
          `${value}:${stepConfig.retries}`,
      },
      "withRetries",
    );
    const pipeline = pipe(source, withRetries({ retries: 3 }));
    assertExact<IsExact<ExtractInput<typeof pipeline>, any>>();
    assertExact<IsExact<ExtractOutput<typeof pipeline>, string>>();
  });

  it("explicit-typed withConfig handler composes in pipe", () => {
    const source = createHandler(
      {
        handle: async () => "hello",
      },
      "source",
    );
    const transform = createHandlerWithConfig<string, number, { n: number }>(
      {
        handle: async ({ value, stepConfig }) => value.length + stepConfig.n,
      },
      "transform",
    );
    const pipeline = pipe(source, transform({ n: 10 }));
    assertExact<IsExact<ExtractInput<typeof pipeline>, any>>();
    assertExact<IsExact<ExtractOutput<typeof pipeline>, number>>();
  });
});
