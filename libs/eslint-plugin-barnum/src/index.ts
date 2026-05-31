import exportedHandler from "./rules/exported-handler.js";
import requireTypeParams from "./rules/require-type-params.js";

const plugin = {
  rules: {
    "exported-handler": exportedHandler,
    "require-type-params": requireTypeParams,
  },
};

export default plugin;
