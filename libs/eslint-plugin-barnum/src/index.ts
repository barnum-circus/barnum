import exportedHandler from "./rules/exported-handler.js";

const plugin = {
  rules: {
    "exported-handler": exportedHandler,
  },
};

export default plugin;
