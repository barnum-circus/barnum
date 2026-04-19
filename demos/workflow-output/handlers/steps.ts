import { createHandler } from "@barnum/barnum/runtime";
import { z } from "zod";

export const double = createHandler(
  {
    inputValidator: z.number(),
    outputValidator: z.number(),
    handle: async ({ value }) => {
      console.error(`[double] ${value} * 2 = ${value * 2}`);
      return value * 2;
    },
  },
  "double",
);

export const addLabel = createHandler(
  {
    inputValidator: z.number(),
    outputValidator: z.object({ label: z.string(), value: z.number() }),
    handle: async ({ value }) => {
      const result = { label: `result-${value}`, value };
      console.error(`[addLabel] ${JSON.stringify(result)}`);
      return result;
    },
  },
  "addLabel",
);
