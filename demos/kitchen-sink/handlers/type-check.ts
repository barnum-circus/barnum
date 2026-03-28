// TypeCheck: run tsc and return any type errors found.
//
// In a real workflow this would exec `tsc --noEmit` and parse the output.
// Here we return an empty array (no errors) so the fix loop exits
// immediately. A real implementation would look like barnum-demo's
// type-check.ts which shells out to tsc.

import { createHandler } from "@barnum/barnum/src/handler.js";

export type TypeError = {
  file: string;
  message: string;
};

// typeCheck operates on the filesystem, not on a pipeline value.
// No stepValueValidator → Handler<never, TypeError[]>.
export default createHandler({
  handle: async (): Promise<TypeError[]> => {
    console.error("[type-check] Running tsc --noEmit...");
    // Stub: always clean. In reality, parse tsc output.
    return [];
  },
});
