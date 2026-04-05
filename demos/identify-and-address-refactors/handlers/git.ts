// Generic git operation handlers — reusable across workflows.
//
// createWorktree: create a git worktree for a branch
// deleteWorktree: remove a git worktree
// createPR: push branch and create a pull request (simulated for demo)

import { createHandler } from "@barnum/barnum";
import { spawnSync } from "node:child_process";
import { z } from "zod";
import { baseDir } from "./lib.js";

/** Run a git command in the repo root. Returns stdout. */
function git(args: string[], cwd?: string): string {
  const result = spawnSync("git", args, {
    encoding: "utf-8",
    cwd: cwd ?? baseDir,
    timeout: 30_000,
  });
  if (result.status !== 0) {
    throw new Error(`git ${args[0]} failed: ${result.stderr}`);
  }
  return result.stdout.trim();
}

// --- Create worktree ---

export const createWorktree = createHandler({
  inputValidator: z.object({ branch: z.string() }),
  outputValidator: z.object({ worktreePath: z.string(), branch: z.string() }),
  handle: async ({ value }) => {
    const worktreePath = `/tmp/barnum-demo-worktrees/${value.branch.replace(/\//g, "-")}`;
    console.error(`[create-worktree] Creating worktree: ${value.branch} at ${worktreePath}`);

    // Find repo root from baseDir
    const repoRoot = git(["-C", baseDir, "rev-parse", "--show-toplevel"]);

    // Create the worktree branch from current HEAD
    git(["-C", repoRoot, "worktree", "add", worktreePath, "-b", value.branch], repoRoot);

    console.error(`[create-worktree] Created ${worktreePath}`);
    return { worktreePath, branch: value.branch };
  },
}, "createWorktree");

// --- Delete worktree ---

export const deleteWorktree = createHandler({
  inputValidator: z.object({ worktreePath: z.string(), branch: z.string() }),
  handle: async ({ value }) => {
    console.error(`[delete-worktree] Removing ${value.worktreePath}`);

    const repoRoot = git(["-C", baseDir, "rev-parse", "--show-toplevel"]);

    // Remove the worktree
    try {
      git(["-C", repoRoot, "worktree", "remove", value.worktreePath, "--force"]);
    } catch {
      console.error(`[delete-worktree] Warning: worktree removal failed, cleaning up manually`);
      spawnSync("rm", ["-rf", value.worktreePath], { encoding: "utf-8" });
      git(["-C", repoRoot, "worktree", "prune"]);
    }

    // Delete the branch
    const branch = value.worktreePath.split("/").pop() ?? "";
    try {
      git(["-C", repoRoot, "branch", "-D", branch]);
    } catch {
      // Branch may not exist or may already be deleted
    }

    console.error(`[delete-worktree] Cleaned up ${value.worktreePath}`);
  },
}, "deleteWorktree");

// --- Create PR ---

// For the demo, this simulates PR creation (no real GitHub remote needed).
export const createPR = createHandler({
  inputValidator: z.object({
    branch: z.string(),
    title: z.string(),
    body: z.string(),
  }),
  outputValidator: z.object({ prUrl: z.string() }),
  handle: async ({ value }) => {
    console.error(`[create-pr] Would create PR for ${value.branch}: ${value.title}`);
    console.error(`[create-pr] Body: ${value.body.slice(0, 100)}...`);

    // In production: git push origin <branch> && gh pr create --title "..." --body "..."
    // For demo: simulate PR URL
    const prUrl = `https://github.com/demo/repo/pull/${Date.now() % 1000}`;
    console.error(`[create-pr] Simulated PR: ${prUrl}`);
    return { prUrl };
  },
}, "createPR");
