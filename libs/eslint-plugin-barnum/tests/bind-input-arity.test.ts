import { RuleTester } from "eslint";
import tseslint from "typescript-eslint";
import { describe, it } from "vitest";
import rule from "../src/rules/bind-input-arity.js";

const ruleTester = new RuleTester({
  languageOptions: {
    ecmaVersion: 2023,
    sourceType: "module",
    parser: tseslint.parser,
  },
});

describe("bind-input-arity", () => {
  it("requires bindInput callback to have a parameter", () => {
    ruleTester.run("bind-input-arity", rule, {
      valid: [
        // Standalone with parameter
        `bindInput<string, number>((input) => input)`,
        // Postfix with parameter
        `x.bindInput<never>((input) => input)`,
        // Underscore-prefixed is fine
        `bindInput<string, null>((_input) => drop)`,
        // Postfix underscore
        `x.bindInput<string>((_ref) => constant("done"))`,
      ],
      invalid: [
        // Standalone with no parameter
        {
          code: `bindInput<string, null>(() => drop)`,
          errors: [{ messageId: "missingParam" }],
        },
        // Postfix with no parameter
        {
          code: `x.bindInput<never>(() => recur)`,
          errors: [{ messageId: "missingParam" }],
        },
      ],
    });
  });

  it("ignores unrelated function calls", () => {
    ruleTester.run("bind-input-arity", rule, {
      valid: [
        `someOtherFunction(() => 42)`,
        `x.map(() => 42)`,
        `x.then(() => done)`,
      ],
      invalid: [],
    });
  });
});
