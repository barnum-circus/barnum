import barnumPlugin from "@barnum/eslint-plugin";

export default [
  ...barnumPlugin.configs.recommended,
  {
    // Type-only test files define handlers inline without exporting —
    // they're never executed by the runtime, only typechecked.
    files: ["tests/handler.test.ts"],
    rules: {
      "barnum/exported-handler": "off",
    },
  },
];
