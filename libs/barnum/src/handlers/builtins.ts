import { z } from "zod";
import { createHandler } from "../core.js";

export const constant = createHandler(
  {
    stepConfigValidator: z.object({ value: z.unknown() }),
    handle: async ({ stepConfig }) => stepConfig.value,
  },
  "constant",
);

export const range = createHandler(
  {
    stepConfigValidator: z.object({ start: z.number(), end: z.number() }),
    handle: async ({ stepConfig }) => {
      const result: number[] = [];
      for (let i = stepConfig.start; i < stepConfig.end; i++) {
        result.push(i);
      }
      return result;
    },
  },
  "range",
);

export const drop = createHandler(
  {
    handle: async () => undefined,
  },
  "drop",
);
