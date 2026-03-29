// Handlers for the identify-and-address-refactors workflow.
//
// Discovery: listTargetFiles, analyze
// Data shaping: deriveBranch, preparePRInput
// Implementation: implement, commit
// Review loop: judgeRefactor, classifyJudgment, applyFeedback

import { createHandler } from "@barnum/barnum/src/handler.js";
import { z } from "zod";

// --- Types ---

export type Refactor = {
  file: string;
  description: string;
  scope: "function" | "module" | "cross-file";
};

export type JudgmentResult =
  | { approved: true }
  | { approved: false; instructions: string };

export type ClassifyJudgmentResult =
  | { kind: "Approved" }
  | { kind: "NeedsWork"; instructions: string };

// --- Discovery ---

// In production: glob folder for source files, filter by gitignore/node_modules.
export const listTargetFiles = createHandler({
  stepValueValidator: z.object({ folder: z.string() }),
  handle: async ({ value }) => {
    console.error(`[list-target-files] Scanning ${value.folder}/ ...`);
    return [
      { file: `${value.folder}/src/api.ts` },
      { file: `${value.folder}/src/utils.ts` },
      { file: `${value.folder}/src/components/Button.tsx` },
    ];
  },
}, "listTargetFiles");

// In production:
//   Prompt: "Analyze the following file and identify specific, independent
//   refactoring opportunities. For each, describe the change and why it
//   improves the code. Return a JSON array of refactors."
export const analyze = createHandler({
  stepValueValidator: z.object({ file: z.string() }),
  handle: async ({ value }): Promise<Refactor[]> => {
    console.error(`[analyze] Analyzing ${value.file} for refactoring opportunities...`);
    return [
      {
        file: value.file,
        description: `Extract error handling into shared utility in ${value.file}`,
        scope: "function",
      },
      {
        file: value.file,
        description: `Replace callback pattern with async/await in ${value.file}`,
        scope: "module",
      },
    ];
  },
}, "analyze");

// --- Data shaping ---

// Derive a git branch name from a refactor description.
export const deriveBranch = createHandler({
  stepValueValidator: z.object({ description: z.string() }),
  handle: async ({ value }) => ({
    branch: `refactor/${value.description.toLowerCase().replace(/[^a-z0-9]+/g, "-").slice(0, 40)}`,
  }),
}, "deriveBranch");

// Prepare PR metadata from refactor context.
export const preparePRInput = createHandler({
  stepValueValidator: z.object({
    branch: z.string(),
    description: z.string(),
  }),
  handle: async ({ value }) => ({
    branch: value.branch,
    title: `Refactor: ${value.description.slice(0, 60)}`,
    body: `Automated refactor:\n\n${value.description}`,
  }),
}, "preparePRInput");

// --- Implementation ---

// In production:
//   Prompt: "You are working in {worktreePath}. Implement the following
//   refactor: {description}. Make minimal, focused changes."
export const implement = createHandler({
  stepValueValidator: z.object({
    worktreePath: z.string(),
    description: z.string(),
  }),
  handle: async ({ value }) => {
    console.error(`[implement] Applying refactor in ${value.worktreePath}: ${value.description}`);
  },
}, "implement");

// In production: `git -C {worktreePath} add -A && git commit -m "{message}"`.
export const commit = createHandler({
  stepValueValidator: z.object({ worktreePath: z.string() }),
  handle: async ({ value }) => {
    console.error(`[commit] Committing in ${value.worktreePath}`);
  },
}, "commit");

// --- Review loop ---

// In production:
//   Prompt: "Review the changes on the current branch. Evaluate whether
//   this refactor is correct, complete, and follows best practices.
//   If improvements are needed, provide specific instructions."
export const judgeRefactor = createHandler({
  handle: async (): Promise<JudgmentResult> => {
    console.error("[judge-refactor] Reviewing changes...");
    return { approved: true };
  },
}, "judgeRefactor");

// Pure data transform: { approved, instructions? } → discriminated union for branch.
export const classifyJudgment = createHandler({
  stepValueValidator: z.union([
    z.object({ approved: z.literal(true) }),
    z.object({ approved: z.literal(false), instructions: z.string() }),
  ]),
  handle: async ({ value: judgment }): Promise<ClassifyJudgmentResult> => {
    if (judgment.approved) {
      console.error("[classify-judgment] Approved");
      return { kind: "Approved" };
    }
    console.error(`[classify-judgment] Needs work: ${judgment.instructions}`);
    return { kind: "NeedsWork", instructions: judgment.instructions };
  },
}, "classifyJudgment");

// In production:
//   Prompt: "The reviewer provided the following feedback on your refactor:
//   {instructions}. Apply these changes to the files in the worktree."
export const applyFeedback = createHandler({
  stepValueValidator: z.string(),
  handle: async ({ value: instructions }) => {
    console.error(`[apply-feedback] Applying: ${instructions}`);
  },
}, "applyFeedback");
