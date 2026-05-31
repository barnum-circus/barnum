import exportedHandler from "./rules/exported-handler.js";
import requireCallbackParams from "./rules/require-callback-params.js";
import requireTypeParams from "./rules/require-type-params.js";

const plugin = {
  rules: {
    "exported-handler": exportedHandler,
    "require-callback-params": requireCallbackParams,
    "require-type-params": requireTypeParams,
  },
};

export default plugin;
