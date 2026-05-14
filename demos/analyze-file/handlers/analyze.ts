import { createHandler, taggedUnionSchema } from "@barnum/barnum/runtime";
import type { Result } from "@barnum/barnum/pipeline";
import { z } from "zod";

const resultSchema = taggedUnionSchema("Result", {
  Ok: z.string(),
  Err: z.string(),
});

type AnalysisResult = Result<string, string>;

/** Simulates a flaky analysis that fails on the first attempt. */
let classComponentAttempts = 0;
export const analyzeClassComponents = createHandler(
  {
    inputValidator: z.string(),
    outputValidator: resultSchema,
    handle: async ({ value: file }): Promise<AnalysisResult> => {
      classComponentAttempts++;
      console.error(
        `[analyzeClassComponents] attempt ${classComponentAttempts} on ${file}...`,
      );
      if (classComponentAttempts < 2) {
        return { kind: "Result.Err" as const, value: "transient failure" };
      }
      return {
        kind: "Result.Ok" as const,
        value: `${file}: no class components found`,
      };
    },
  },
  "analyzeClassComponents",
);

export const analyzeImpossibleStates = createHandler(
  {
    inputValidator: z.string(),
    outputValidator: resultSchema,
    handle: async ({ value: file }): Promise<AnalysisResult> => {
      console.error(
        `[analyzeImpossibleStates] Scanning ${file} for impossible states...`,
      );
      return {
        kind: "Result.Ok" as const,
        value: `${file}: 2 impossible states found`,
      };
    },
  },
  "analyzeImpossibleStates",
);

export const analyzeErrorHandling = createHandler(
  {
    inputValidator: z.string(),
    outputValidator: resultSchema,
    handle: async ({ value: file }): Promise<AnalysisResult> => {
      console.error(
        `[analyzeErrorHandling] Scanning ${file} for error handling issues...`,
      );
      return {
        kind: "Result.Ok" as const,
        value: `${file}: 1 unhandled error path`,
      };
    },
  },
  "analyzeErrorHandling",
);
