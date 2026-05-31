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
  it("enforces type parameters on loop", () => {
    ruleTester.run("require-type-params", rule, {
      valid: [
        // 1 type param (TIn) — minimum
        `loop<string>((recur, done) => done)`,
        // 2 type params (TIn, TOut)
        `loop<string, number>((recur, done) => done)`,
        // never is valid for output position
        `loop<string, never>((recur, done) => recur)`,
        // void is valid
        `loop<void, null>((recur, done) => done)`,
      ],
      invalid: [
        // No type params
        {
          code: `loop((recur, done) => done)`,
          errors: [{ messageId: "missingTypeParams" }],
        },
        // unknown in TIn (input position)
        {
          code: `loop<unknown>((recur, done) => done)`,
          errors: [{ messageId: "unknownInInput" }],
        },
        // any in TOut (output position)
        {
          code: `loop<string, any>((recur, done) => done)`,
          errors: [{ messageId: "anyInOutput" }],
        },
        // unknown in TIn with TOut provided
        {
          code: `loop<unknown, number>((recur, done) => done)`,
          errors: [{ messageId: "unknownInInput" }],
        },
        // both violations
        {
          code: `loop<unknown, any>((recur, done) => done)`,
          errors: [
            { messageId: "unknownInInput" },
            { messageId: "anyInOutput" },
          ],
        },
      ],
    });
  });

  it("enforces type parameters on earlyReturn", () => {
    ruleTester.run("require-type-params", rule, {
      valid: [
        // 1 type param (TEarlyReturn)
        `earlyReturn<string>((ret) => ret)`,
        // 3 type params
        `earlyReturn<string, number, boolean>((ret) => ret)`,
        // never in output positions is fine
        `earlyReturn<never>((ret) => ret)`,
      ],
      invalid: [
        // No type params
        {
          code: `earlyReturn((ret) => ret)`,
          errors: [{ messageId: "missingTypeParams" }],
        },
        // any in TEarlyReturn (output position)
        {
          code: `earlyReturn<any>((ret) => ret)`,
          errors: [{ messageId: "anyInOutput" }],
        },
        // unknown in TIn (input position, param 2)
        {
          code: `earlyReturn<string, unknown>((ret) => ret)`,
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
