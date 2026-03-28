import { createHandler } from "@barnum/barnum/src/handler.js";

export default createHandler({
  handle: async () => {
    console.error("[initialize] Setting up project...");
    return { project: "my-app", version: "1.0.0" };
  },
});
