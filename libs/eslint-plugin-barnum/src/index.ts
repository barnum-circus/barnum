import exportedHandler from "./rules/exported-handler.js";
import requireCallbackParams from "./rules/require-callback-params.js";
import requireTypeParams from "./rules/require-type-params.js";
import tseslint from "typescript-eslint";

const plugin = {
  meta: {
    name: "@barnum/eslint-plugin",
  },
  rules: {
    "exported-handler": exportedHandler,
    "require-callback-params": requireCallbackParams,
    "require-type-params": requireTypeParams,
  },
  configs: {
    get recommended() {
      return [
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
            barnum: plugin,
          },
          rules: {
            "barnum/exported-handler": "error",
            "barnum/require-callback-params": "error",
            "barnum/require-type-params": "error",
          },
        },
      ];
    },
  },
};

export default plugin;
