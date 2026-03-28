import { createHandler } from "@barnum/barnum/src/handler.js";
import { z } from "zod";

let callCount = 0;

export default createHandler({
  stepValueValidator: z.object({ attempt: z.number() }),
  handle: async ({ value }) => {
    callCount++;
    console.error(`[poll-status] Attempt ${value.attempt} (call #${callCount})...`);

    // Simulate: succeed on the 3rd attempt
    if (value.attempt >= 3) {
      return { kind: "Break" as const, value: { ready: true, attempts: value.attempt } };
    }
    return { kind: "Continue" as const, value: { attempt: value.attempt + 1 } };
  },
});
