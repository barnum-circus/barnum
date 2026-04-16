import { createHandler } from "@barnum/barnum/runtime";
import { z } from "zod";

export const analyzeClassComponents = createHandler(
  {
    inputValidator: z.string(),
    outputValidator: z.string(),
    handle: async ({ value: file }) => {
      console.error(
        `[analyzeClassComponents] Scanning ${file} for class components...`,
      );
      return `${file}: no class components found`;
    },
  },
  "analyzeClassComponents",
);

export const analyzeImpossibleStates = createHandler(
  {
    inputValidator: z.string(),
    outputValidator: z.string(),
    handle: async ({ value: file }) => {
      console.error(
        `[analyzeImpossibleStates] Scanning ${file} for impossible states...`,
      );
      return `${file}: 2 impossible states found`;
    },
  },
  "analyzeImpossibleStates",
);

export const analyzeErrorHandling = createHandler(
  {
    inputValidator: z.string(),
    outputValidator: z.string(),
    handle: async ({ value: file }) => {
      console.error(
        `[analyzeErrorHandling] Scanning ${file} for error handling issues...`,
      );
      return `${file}: 1 unhandled error path`;
    },
  },
  "analyzeErrorHandling",
);
