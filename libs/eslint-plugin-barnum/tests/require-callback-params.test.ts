import { RuleTester } from "eslint";
import tseslint from "typescript-eslint";
import { describe, it } from "vitest";
import rule from "../src/rules/require-callback-params.js";

const ruleTester = new RuleTester({
  languageOptions: {
    ecmaVersion: 2023,
    sourceType: "module",
    parser: tseslint.parser,
  },
});

describe("require-callback-params", () => {
  it("requires bindInput callback to have a parameter", () => {
    ruleTester.run("require-callback-params", rule, {
      valid: [
        `bindInput<string, number>((input) => input)`,
        `x.bindInput<never>((input) => input)`,
        `bindInput<string, null>((_input) => drop)`,
      ],
      invalid: [
        {
          code: `bindInput<string, null>(() => drop)`,
          errors: [{ messageId: "missingParams" }],
        },
        {
          code: `x.bindInput<never>(() => recur)`,
          errors: [{ messageId: "missingParams" }],
        },
      ],
    });
  });

  it("requires loop callback to have at least one parameter", () => {
    ruleTester.run("require-callback-params", rule, {
      valid: [
        `loop<void>((recur) => recur)`,
        `loop<void>((recur, done) => done)`,
        `loop<string, number>((recur, done) => done)`,
      ],
      invalid: [
        {
          code: `loop<void>(() => drop)`,
          errors: [{ messageId: "missingParams" }],
        },
      ],
    });
  });

  it("requires earlyReturn callback to have at least one parameter", () => {
    ruleTester.run("require-callback-params", rule, {
      valid: [
        `earlyReturn<string>((ret) => ret)`,
        `earlyReturn<string>((ret) => pipe(constant("x"), ret))`,
      ],
      invalid: [
        {
          code: `earlyReturn<string>(() => constant("x"))`,
          errors: [{ messageId: "missingParams" }],
        },
      ],
    });
  });

  it("ignores unrelated function calls", () => {
    ruleTester.run("require-callback-params", rule, {
      valid: [
        `someOtherFunction(() => 42)`,
        `x.map(() => 42)`,
        `x.then(() => done)`,
      ],
      invalid: [],
    });
  });
});
