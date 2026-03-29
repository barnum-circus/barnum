// Handlers for the identify-and-address-refactors workflow.
//
// Discovery: listTargetFiles, analyze
// Worktree lifecycle (RAII): createWorktree, deleteWorktree
// Implementation: implement, commit
// Review loop: judgeRefactor, classifyJudgment, applyFeedback
// Delivery: createPR

import { createHandler } from "@barnum/barnum/src/handler.js";
import { z } from "zod";

// --- Types ---

export type Refactor = {
  file: string;
  description: string;
  scope: "function" | "module" | "cross-file";
};

export type WorktreeContext = {
  worktreePath: string;
  branch: string;
  refactorDescription: string;
};

export type JudgmentResult =
  | { approved: true }
  | { approved: false; instructions: string };

export type ClassifyJudgmentResult =
  | { kind: "Approved" }
  | { kind: "NeedsWork"; instructions: string };

export type PRResult = {
  prUrl: string;
  worktreePath: string;
};

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

// --- Worktree lifecycle (RAII pair) ---

// Create half: generate branch name, `git worktree add <path> -b <branch>`.
export const createWorktree = createHandler({
  stepValueValidator: z.object({
    file: z.string(),
    description: z.string(),
    scope: z.enum(["function", "module", "cross-file"]),
  }),
  handle: async ({ value: refactor }): Promise<WorktreeContext> => {
    const slug = refactor.description
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .slice(0, 40);
    const branch = `refactor/${slug}`;
    const worktreePath = `/tmp/worktrees/${slug}`;
    console.error(`[create-worktree] ${branch} at ${worktreePath}`);
    return { worktreePath, branch, refactorDescription: refactor.description };
  },
}, "createWorktree");

// Dispose half: `git worktree remove {worktreePath}`, optionally `git branch -D`.
export const deleteWorktree = createHandler({
  stepValueValidator: z.object({
    prUrl: z.string(),
    worktreePath: z.string(),
  }),
  handle: async ({ value }) => {
    console.error(`[delete-worktree] Removing ${value.worktreePath} (PR: ${value.prUrl})`);
  },
}, "deleteWorktree");

// --- Implementation ---

// In production:
//   Prompt: "You are working in {worktreePath}. Implement the following
//   refactor: {refactorDescription}. Make minimal, focused changes.
//   Edit only the files necessary."
export const implement = createHandler({
  stepValueValidator: z.object({
    worktreePath: z.string(),
    branch: z.string(),
    refactorDescription: z.string(),
  }),
  handle: async ({ value: ctx }): Promise<WorktreeContext> => {
    console.error(`[implement] Applying refactor in ${ctx.worktreePath}: ${ctx.refactorDescription}`);
    return ctx;
  },
}, "implement");

// In production: `git -C {worktreePath} add -A && git commit -m "{message}"`.
export const commit = createHandler({
  stepValueValidator: z.object({
    worktreePath: z.string(),
    branch: z.string(),
    refactorDescription: z.string(),
  }),
  handle: async ({ value: ctx }): Promise<WorktreeContext> => {
    console.error(`[commit] Committing in ${ctx.worktreePath} on ${ctx.branch}`);
    return ctx;
  },
}, "commit");

// --- Review loop ---

// In production:
//   Prompt: "Review the changes on branch {branch} in {worktreePath}.
//   Evaluate whether this refactor is correct, complete, and follows
//   best practices. If improvements are needed, provide specific
//   instructions. Respond with { approved: true } or
//   { approved: false, instructions: '...' }."
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
//   {instructions}
//   Apply these changes to the files in the worktree."
export const applyFeedback = createHandler({
  stepValueValidator: z.string(),
  handle: async ({ value: instructions }) => {
    console.error(`[apply-feedback] Applying: ${instructions}`);
  },
}, "applyFeedback");

// --- Delivery ---

// In production: `git push origin {branch}`, then `gh pr create --title "..." --body "..."`.
export const createPR = createHandler({
  handle: async (): Promise<PRResult> => {
    console.error("[create-pr] Pushing branch and creating PR...");
    return {
      prUrl: "https://github.com/org/repo/pull/42",
      worktreePath: "/tmp/worktrees/current",
    };
  },
}, "createPR");
