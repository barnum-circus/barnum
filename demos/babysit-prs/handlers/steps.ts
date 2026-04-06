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
      z.object({ kind: z.literal("ChecksPassed"), value: z.number() }),
      z.object({ kind: z.literal("Landed"), value: z.number() }),
    ]),
    handle: async ({ value: pr }) => {
      console.error(`[checkPR] Checking PR #${pr}...`);
      await sleep(2_000 + Math.random() * 8_000);

      const roll = Math.random();
      if (roll < 0.5) {
        const error = ciErrors[Math.floor(Math.random() * ciErrors.length)]!;
        console.error(`[checkPR] PR #${pr}: checks failed — ${error}`);
        return { kind: "ChecksFailed" as const, value: { pr, error } };
      }

      if (roll < 0.8) {
        console.error(`[checkPR] PR #${pr}: checks passed`);
        return { kind: "ChecksPassed" as const, value: pr };
      }

      console.error(`[checkPR] PR #${pr}: already landed`);
      return { kind: "Landed" as const, value: pr };
    },
  },
  "checkPR",
);

// --- Fix CI failures (fake LLM) ---

export const fixIssues = createHandler(
  {
    inputValidator: z.object({ pr: z.number(), error: z.string() }),
    outputValidator: z.number(),
    handle: async ({ value }) => {
      console.error(`[fixIssues] Fixing PR #${value.pr}: ${value.error}`);
      await sleep(1_000 + Math.random() * 2_000);
      console.error(`[fixIssues] Fix pushed for PR #${value.pr}`);
      return value.pr;
    },
  },
  "fixIssues",
);

// --- Land a PR (fake merge) ---

export const landPR = createHandler(
  {
    inputValidator: z.number(),
    outputValidator: z.null(),
    handle: async ({ value: pr }) => {
      console.error(`[landPR] Merging PR #${pr}...`);
      await sleep(500 + Math.random() * 1_000);
      console.error(`[landPR] PR #${pr} merged`);
      return null;
    },
  },
  "landPR",
);

// --- Filter done PRs and decide whether to loop ---

export const classifyRemaining = createHandler(
  {
    inputValidator: z.array(z.union([z.number(), z.null()])),
    outputValidator: z.discriminatedUnion("kind", [
      z.object({ kind: z.literal("HasPRs"), value: z.array(z.number()) }),
      z.object({ kind: z.literal("AllDone"), value: z.null() }),
    ]),
    handle: async ({ value }) => {
      const remaining = value.filter((x): x is number => x !== null);
      if (remaining.length === 0) {
        console.error("[classifyRemaining] All PRs resolved");
        return { kind: "AllDone" as const, value: null };
      }
      console.error(
        `[classifyRemaining] ${remaining.length} PR(s) still need attention: #${remaining.join(", #")}`,
      );
      return { kind: "HasPRs" as const, value: remaining };
    },
  },
  "classifyRemaining",
);
