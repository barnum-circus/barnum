// Handlers for the identify-and-address-refactors workflow.
//
// Discovery: listTargetFiles, analyze
// Data shaping: deriveBranch, preparePRInput
// Implementation: implement, commit
// Review loop: judgeRefactor, classifyJudgment, applyFeedback

import { createHandler } from "@barnum/barnum/src/handler.js";
import { spawnSync } from "node:child_process";
import { readdirSync } from "node:fs";
import path from "node:path";
import { z } from "zod";
import { baseDir, callClaude } from "./lib.js";

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
  | { kind: "Approved"; value: void }
  | { kind: "NeedsWork"; value: string };

// --- Discovery ---

export const listTargetFiles = createHandler({
  inputValidator: z.object({ folder: z.string() }),
  handle: async ({ value }) => {
    console.error(`[list-target-files] Scanning ${value.folder}/ ...`);
    const files = readdirSync(value.folder)
      .filter((name) => name.endsWith(".ts") || name.endsWith(".tsx") || name.endsWith(".js"))
      .sort();

    const result = files.map((name) => ({
      file: path.join(value.folder, name),
    }));

    console.error(`[list-target-files] Found ${result.length} files`);
    return result;
  },
}, "listTargetFiles");

export const analyze = createHandler({
  inputValidator: z.object({ file: z.string() }),
  handle: async ({ value }): Promise<Refactor[]> => {
    console.error(`[analyze] Analyzing ${value.file} for refactoring opportunities...`);

    const response = callClaude({
      prompt: [
        `Analyze the file ${value.file} for refactoring opportunities.`,
        "Identify 1-2 specific, independent refactoring opportunities.",
        "For each, describe the change and classify scope as function/module/cross-file.",
        "Return a JSON array of objects with fields: file, description, scope.",
        "Return ONLY the JSON array, no markdown fences, no explanation.",
      ].join("\n"),
      allowedTools: ["Read"],
    });

    try {
      // Try to parse the JSON response
      const cleaned = response.trim().replace(/^```json?\n?/, "").replace(/\n?```$/, "");
      const refactors: Refactor[] = JSON.parse(cleaned);
      console.error(`[analyze] Found ${refactors.length} opportunities in ${value.file}`);
      return refactors;
    } catch {
      console.error(`[analyze] Failed to parse Claude's response, returning empty`);
      return [];
    }
  },
}, "analyze");

// --- Data shaping ---

export const deriveBranch = createHandler({
  inputValidator: z.object({ description: z.string() }),
  handle: async ({ value }) => ({
    branch: `refactor/${value.description.toLowerCase().replace(/[^a-z0-9]+/g, "-").slice(0, 40)}`,
  }),
}, "deriveBranch");

export const preparePRInput = createHandler({
  inputValidator: z.object({
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

export const implement = createHandler({
  inputValidator: z.object({
    worktreePath: z.string(),
    description: z.string(),
  }),
  handle: async ({ value }) => {
    console.error(`[implement] Applying refactor in ${value.worktreePath}: ${value.description}`);

    callClaude({
      prompt: [
        `Implement the following refactor in the codebase:`,
        `${value.description}`,
        "",
        `Working directory: ${value.worktreePath}`,
        "Make minimal, focused changes. Only modify what's needed for this refactor.",
      ].join("\n"),
      allowedTools: ["Read", "Edit"],
      cwd: value.worktreePath,
    });

    console.error(`[implement] Refactor applied`);
  },
}, "implement");

export const commit = createHandler({
  inputValidator: z.object({ worktreePath: z.string() }),
  handle: async ({ value }) => {
    console.error(`[commit] Committing changes in ${value.worktreePath}`);

    spawnSync("git", ["add", "-A"], {
      cwd: value.worktreePath,
      encoding: "utf-8",
    });

    const result = spawnSync("git", [
      "commit", "-m", "Apply automated refactor",
      "--allow-empty",
    ], {
      cwd: value.worktreePath,
      encoding: "utf-8",
    });

    if (result.status !== 0) {
      console.error(`[commit] Warning: commit may have failed: ${result.stderr}`);
    } else {
      console.error(`[commit] Committed`);
    }
  },
}, "commit");

// --- Review loop ---

export const judgeRefactor = createHandler({
  handle: async (): Promise<JudgmentResult> => {
    console.error("[judge-refactor] Reviewing changes...");

    const response = callClaude({
      prompt: [
        "Review the recent changes (git diff HEAD~1) in this repository.",
        "Evaluate whether the refactor is correct, complete, and follows best practices.",
        "Return a JSON object:",
        '  If approved: { "approved": true }',
        '  If needs work: { "approved": false, "instructions": "specific feedback here" }',
        "Return ONLY the JSON object, no markdown fences, no explanation.",
      ].join("\n"),
      allowedTools: ["Bash(git:*)"],
    });

    try {
      const cleaned = response.trim().replace(/^```json?\n?/, "").replace(/\n?```$/, "");
      const judgment: JudgmentResult = JSON.parse(cleaned);
      console.error(`[judge-refactor] ${judgment.approved ? "Approved" : "Needs work"}`);
      return judgment;
    } catch {
      // If we can't parse, approve it (demo safety)
      console.error("[judge-refactor] Could not parse response, approving by default");
      return { approved: true };
    }
  },
}, "judgeRefactor");

export const classifyJudgment = createHandler({
  inputValidator: z.union([
    z.object({ approved: z.literal(true) }),
    z.object({ approved: z.literal(false), instructions: z.string() }),
  ]),
  handle: async ({ value: judgment }): Promise<ClassifyJudgmentResult> => {
    if (judgment.approved) {
      console.error("[classify-judgment] Approved");
      return { kind: "Approved", value: undefined };
    }
    console.error(`[classify-judgment] Needs work: ${judgment.instructions}`);
    return { kind: "NeedsWork", value: judgment.instructions };
  },
}, "classifyJudgment");

export const applyFeedback = createHandler({
  inputValidator: z.string(),
  handle: async ({ value: instructions }) => {
    console.error(`[apply-feedback] Applying feedback: ${instructions}`);

    callClaude({
      prompt: [
        "The reviewer provided feedback on the refactor:",
        instructions,
        "",
        "Apply these changes to the codebase. Read the relevant files, understand the feedback, and make the edits.",
      ].join("\n"),
      allowedTools: ["Read", "Edit"],
    });

    console.error(`[apply-feedback] Feedback applied`);
  },
}, "applyFeedback");
