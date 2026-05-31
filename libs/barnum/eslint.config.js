import barnumPlugin from "@barnum/eslint-plugin";
import tseslint from "typescript-eslint";

export default [
  {
    files: ["**/*.ts"],
    linterOptions: {
      reportUnusedDisableDirectives: "off",
    },
    languageOptions: {
      parser: tseslint.parser,
    },
    plugins: {
      "@typescript-eslint": tseslint.plugin,
      barnum: barnumPlugin,
    },
    rules: {
      "barnum/bind-input-arity": "error",
      "barnum/exported-handler": "error",
      "barnum/require-type-params": "error",
    },
  },
  {
    // Type-only test files define handlers inline without exporting —
    // they're never executed by the runtime, only typechecked.
    files: ["tests/handler.test.ts"],
    rules: {
      "barnum/exported-handler": "off",
    },
  },
];
