import { RuleTester } from "eslint";
import tseslint from "typescript-eslint";
import { describe, it } from "vitest";
import rule from "../src/rules/require-type-params.js";

const ruleTester = new RuleTester({
  languageOptions: {
    ecmaVersion: 2023,
    sourceType: "module",
    parser: tseslint.parser,
  },
});

describe("require-type-params", () => {
  it("does not track loop (its defaults are concrete, not `any`)", () => {
    ruleTester.run("require-type-params", rule, {
      valid: [
        // loop is intentionally not covered — bare loop infers a concrete
        // output and still type-checks its body, so no type params are required.
        `loop((recur, done) => done)`,
        `loop<unknown>((recur, done) => done)`,
        `loop<string, any>((recur, done) => done)`,
      ],
      invalid: [],
    });
  });

  it("enforces type parameters on earlyReturn", () => {
    ruleTester.run("require-type-params", rule, {
      valid: [
        // All 3 params — only this prevents TIn/TOut defaulting to `any`
        `earlyReturn<string, number, boolean>((ret) => ret)`,
        // never in output positions is fine
        `earlyReturn<never, number, never>((ret) => ret)`,
      ],
      invalid: [
        // No type params
        {
          code: `earlyReturn((ret) => ret)`,
          errors: [{ messageId: "missingTypeParams" }],
        },
        // Only 1 param — TIn and TOut still default to `any`
        {
          code: `earlyReturn<string>((ret) => ret)`,
          errors: [{ messageId: "missingTypeParams" }],
        },
        // Only 2 params — TOut still defaults to `any`
        {
          code: `earlyReturn<string, number>((ret) => ret)`,
          errors: [{ messageId: "missingTypeParams" }],
        },
        // any in TEarlyReturn (output position)
        {
          code: `earlyReturn<any, number, boolean>((ret) => ret)`,
          errors: [{ messageId: "anyInOutput" }],
        },
        // unknown in TIn (input position, param 2)
        {
          code: `earlyReturn<string, unknown, boolean>((ret) => ret)`,
          errors: [{ messageId: "unknownInInput" }],
        },
        // any in TOut (output position, param 3)
        {
          code: `earlyReturn<string, number, any>((ret) => ret)`,
          errors: [{ messageId: "anyInOutput" }],
        },
      ],
    });
  });

  it("enforces type parameters on bindInput", () => {
    ruleTester.run("require-type-params", rule, {
      valid: [
        // Both params provided
        `bindInput<string, number>((input) => input)`,
        // never as TOut is valid (loop body usage)
        `bindInput<string, never>((input) => input)`,
        // Complex types are fine
        `bindInput<{ file: string }, Result<string, Error>>((input) => input)`,
      ],
      invalid: [
        // No type params
        {
          code: `bindInput((input) => input)`,
          errors: [{ messageId: "missingTypeParams" }],
        },
        // Only 1 type param (TIn only — missing TOut)
        {
          code: `bindInput<string>((input) => input)`,
          errors: [{ messageId: "missingTypeParams" }],
        },
        // unknown in TIn (input position)
        {
          code: `bindInput<unknown, string>((input) => input)`,
          errors: [{ messageId: "unknownInInput" }],
        },
        // any in TOut (output position)
        {
          code: `bindInput<string, any>((input) => input)`,
          errors: [{ messageId: "anyInOutput" }],
        },
      ],
    });
  });

  it("enforces type parameters on postfix .bindInput()", () => {
    ruleTester.run("require-type-params", rule, {
      valid: [
        // Postfix with 1 type param (TOut) — minimum for postfix
        `x.bindInput<string>((input) => input)`,
        // never is valid for output
        `x.bindInput<never>((input) => input)`,
      ],
      invalid: [
        // Postfix with no type params
        {
          code: `x.bindInput((input) => input)`,
          errors: [{ messageId: "missingTypeParams" }],
        },
        // any in TOut (output position)
        {
          code: `x.bindInput<any>((input) => input)`,
          errors: [{ messageId: "anyInOutput" }],
        },
      ],
    });
  });

  it("ignores unrelated function calls", () => {
    ruleTester.run("require-type-params", rule, {
      valid: [
        `someOtherFunction()`,
        `bindInput2<string>((x) => x)`,
        `myLoop((a, b) => a)`,
        `pipe(constant(42), action)`,
        // Method calls to non-tracked names are ignored
        `x.toString()`,
        `x.map((item) => item)`,
        `x.loop()`,
      ],
      invalid: [],
    });
  });
});
