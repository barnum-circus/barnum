import bindInputArity from "./rules/bind-input-arity.js";
import exportedHandler from "./rules/exported-handler.js";
import requireTypeParams from "./rules/require-type-params.js";

const plugin = {
  rules: {
    "bind-input-arity": bindInputArity,
    "exported-handler": exportedHandler,
    "require-type-params": requireTypeParams,
  },
};

export default plugin;
