// Generic git operation handlers — reusable across workflows.
//
// createWorktree: create a git worktree for a branch
// deleteWorktree: remove a git worktree
// createPR: push branch and create a pull request

import { createHandler } from "@barnum/barnum/src/handler.js";
import { z } from "zod";

// --- Create worktree ---

// In production: `git worktree add <path> -b <branch>`.
export const createWorktree = createHandler({
  stepValueValidator: z.object({ branch: z.string() }),
  handle: async ({ value }) => {
    const worktreePath = `/tmp/worktrees/${value.branch.replace(/\//g, "-")}`;
    console.error(`[create-worktree] ${value.branch} at ${worktreePath}`);
    return { worktreePath, branch: value.branch };
  },
}, "createWorktree");

// --- Delete worktree ---

// In production: `git worktree remove <worktreePath>`, optionally `git branch -D`.
export const deleteWorktree = createHandler({
  stepValueValidator: z.object({ worktreePath: z.string() }),
  handle: async ({ value }) => {
    console.error(`[delete-worktree] Removing ${value.worktreePath}`);
  },
}, "deleteWorktree");

// --- Create PR ---

// In production: `git push origin <branch>`, then `gh pr create --title "..." --body "..."`.
export const createPR = createHandler({
  stepValueValidator: z.object({
    branch: z.string(),
    title: z.string(),
    body: z.string(),
  }),
  handle: async ({ value }) => {
    console.error(`[create-pr] Pushing ${value.branch}, creating PR: ${value.title}`);
    return { prUrl: "https://github.com/org/repo/pull/42" };
  },
}, "createPR");
