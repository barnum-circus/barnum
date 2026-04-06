import { createHandler } from "@barnum/barnum";
import { z } from "zod";

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

const ciErrors = [
  "Type error in src/index.ts:42 — Property 'name' does not exist on type '{}'",
  "Lint failure: no-unused-vars in utils/helpers.ts",
  "Test failed: expected 200 but got 500 in auth.test.ts",
  "Build error: Cannot find module './missing-dep'",
];

// --- Check PR status (fake GitHub API) ---

export const checkPR = createHandler(
  {
    inputValidator: z.number(),
    outputValidator: z.discriminatedUnion("kind", [
      z.object({
        kind: z.literal("ChecksFailed"),
        value: z.object({ pr: z.number(), error: z.string() }),
      }),
      z.object({
        kind: z.literal("ChecksPassed"),
        value: z.object({ pr: z.number() }),
      }),
      z.object({
        kind: z.literal("Landed"),
        value: z.object({ pr: z.number() }),
      }),
    ]),
    handle: async ({ value: pr }) => {
      console.error(`[checkPR] Checking PR #${pr}...`);

      const roll = Math.random();
      if (roll < 0.5) {
        const error = ciErrors[Math.floor(Math.random() * ciErrors.length)]!;
        console.error(`[checkPR] PR #${pr}: checks failed — ${error}`);
        return { kind: "ChecksFailed" as const, value: { pr, error } };
      }

      if (roll < 0.8) {
        console.error(`[checkPR] PR #${pr}: checks passed`);
        return { kind: "ChecksPassed" as const, value: { pr } };
      }

      console.error(`[checkPR] PR #${pr}: already landed`);
      return { kind: "Landed" as const, value: { pr } };
    },
  },
  "checkPR",
);

// --- Fix CI failures (fake LLM, side effect only) ---

export const fixIssues = createHandler(
  {
    inputValidator: z.object({ pr: z.number(), error: z.string() }),
    handle: async ({ value }) => {
      console.error(`[fixIssues] Fixing PR #${value.pr}: ${value.error}`);
      console.error(`[fixIssues] Fix pushed for PR #${value.pr}`);
    },
  },
  "fixIssues",
);

// --- Land a PR (fake merge, side effect only) ---

export const landPR = createHandler(
  {
    inputValidator: z.object({ pr: z.number() }),
    handle: async ({ value }) => {
      console.error(`[landPR] Merging PR #${value.pr}...`);
      console.error(`[landPR] PR #${value.pr} merged`);
    },
  },
  "landPR",
);

// --- Filter done PRs and decide whether to loop ---

export const classifyRemaining = createHandler(
  {
    inputValidator: z.array(z.number()),
    outputValidator: z.discriminatedUnion("kind", [
      z.object({ kind: z.literal("HasPRs"), value: z.array(z.number()) }),
      z.object({ kind: z.literal("AllDone"), value: z.null() }),
    ]),
    handle: async ({ value }) => {
      if (value.length === 0) {
        console.error("[classifyRemaining] All PRs resolved");
        return { kind: "AllDone" as const, value: null };
      }
      console.error(
        `[classifyRemaining] ${value.length} PR(s) still need attention: #${value.join(", #")}`,
      );
      console.error("[classifyRemaining] Waiting 10s before retry...");
      await sleep(10_000);
      return { kind: "HasPRs" as const, value };
    },
  },
  "classifyRemaining",
);
