import { createHandler } from "@barnum/barnum/src/handler.js";

export default createHandler({
  handle: async () => {
    console.error("[start-polling] Starting polling loop...");
    return { attempt: 1 };
  },
});
