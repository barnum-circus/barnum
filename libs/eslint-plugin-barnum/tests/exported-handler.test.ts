import { RuleTester } from "eslint";
import { describe, it } from "vitest";
import rule from "../src/rules/exported-handler.js";

const ruleTester = new RuleTester({
  languageOptions: {
    ecmaVersion: 2023,
    sourceType: "module",
  },
});

describe("exported-handler", () => {
  it("passes valid and rejects invalid handler declarations", () => {
    ruleTester.run("exported-handler", rule, {
      valid: [
        // Named export with matching exportName
        `export const setup = createHandler({}, "setup")`,
        `export const build = createHandlerWithConfig({}, "build")`,

        // Default export with no arg (defaults to "default")
        `export default createHandler({})`,

        // Default export with explicit "default"
        `export default createHandler({}, "default")`,

        // Non-handler calls are ignored
        `const x = someOtherFunction({}, "x")`,
        `export const y = notAHandler({}, "y")`,

        // Handler with let/var
        `export let migrate = createHandler({}, "migrate")`,
        `export var deploy = createHandler({}, "deploy")`,
      ],

      invalid: [
        // Unexported handler
        {
          code: `const setup = createHandler({}, "setup")`,
          errors: [{ messageId: "unexported" }],
        },

        // Unexported — assigned to variable, no export
        {
          code: `const factory = createHandlerWithConfig({}, "factory")`,
          errors: [{ messageId: "unexported" }],
        },

        // Name mismatch — export name doesn't match arg
        {
          code: `export const setup = createHandler({}, "build")`,
          errors: [{ messageId: "nameMismatch" }],
        },

        // Name mismatch — createHandlerWithConfig
        {
          code: `export const foo = createHandlerWithConfig({}, "bar")`,
          errors: [{ messageId: "nameMismatch" }],
        },

        // Named export with no exportName arg — defaults to "default" which won't match
        {
          code: `export const setup = createHandler({})`,
          errors: [{ messageId: "nameMismatch" }],
        },

        // Default export with wrong exportName
        {
          code: `export default createHandler({}, "notDefault")`,
          errors: [{ messageId: "defaultMissingArg" }],
        },

        // module.exports style
        {
          code: `module.exports.setup = createHandler({}, "setup")`,
          errors: [{ messageId: "moduleExports" }],
        },

        // exports.foo style
        {
          code: `exports.setup = createHandler({}, "setup")`,
          errors: [{ messageId: "moduleExports" }],
        },

        // Inline in function call — not exported
        {
          code: `runPipeline(createHandler({}, "x"))`,
          errors: [{ messageId: "unexported" }],
        },

        // Returned from function — not exported
        {
          code: `function make() { return createHandler({}, "x") }`,
          errors: [{ messageId: "unexported" }],
        },
      ],
    });
  });
});
