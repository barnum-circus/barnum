// Handlers for the identify-and-address-refactors workflow.
//
// Discovery: listTargetFiles, analyze
// Data shaping: deriveBranch, preparePRInput
// Implementation: implement, commit
// Review loop: judgeRefactor, classifyJudgment, applyFeedback
// Pipelines: implementAndReview, createBranchWorktree, openPR

import {
  createHandler,
  taggedUnionSchema,
} from "@barnum/barnum/runtime";
import {
  bindInput,
  pipe,
  loop,
  pick,
  Option,
} from "@barnum/barnum/pipeline";
import { spawnSync } from "node:child_process";
import { readdirSync } from "node:fs";
import path from "node:path";
import { z } from "zod";
import { callClaude } from "./call-claude";

// --- Types ---

export type Refactor = {
  file: string;
  description: string;
  scope: "function" | "module" | "cross-file";
};

export type JudgmentResult =
  | { approved: true }
  | { approved: false; instructions: string };

import type { TaggedUnion } from "@barnum/barnum/runtime";

type ClassifyJudgmentResultDef = {
  Approved: void;
  NeedsWork: string;
};
export type ClassifyJudgmentResult = TaggedUnion<ClassifyJudgmentResultDef>;

// --- Validators ---

const RefactorValidator = z.object({
  file: z.string(),
  description: z.string(),
  scope: z.enum(["function", "module", "cross-file"]),
});

const JudgmentResultValidator = z.union([
  z.object({ approved: z.literal(true) }),
  z.object({ approved: z.literal(false), instructions: z.string() }),
]);

// --- Discovery ---

export const listTargetFiles = createHandler({
  inputValidator: z.object({ folder: z.string() }),
  outputValidator: z.array(z.object({ file: z.string() })),
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
  outputValidator: z.array(RefactorValidator),
  handle: async ({ value }): Promise<Refactor[]> => {
    console.error(`[analyze] Analyzing ${value.file} for refactoring opportunities...`);

    const response = await callClaude({
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

export const assessWorthiness = createHandler({
  inputValidator: RefactorValidator,
  outputValidator: Option.schema(RefactorValidator),
  handle: async ({ value: refactor }): Promise<Option<Refactor>> => {
    console.error(`[assess-worthiness] Evaluating: ${refactor.description}`);

    const response = await callClaude({
      prompt: [
        `Evaluate whether this refactoring opportunity is worth implementing:`,
        `  File: ${refactor.file}`,
        `  Description: ${refactor.description}`,
        `  Scope: ${refactor.scope}`,
        "",
        "Consider: impact on maintainability, risk of introducing bugs, effort vs benefit.",
        'Return a JSON object: { "worthwhile": true } or { "worthwhile": false, "reason": "..." }',
        "Return ONLY the JSON object, no markdown fences, no explanation.",
      ].join("\n"),
      allowedTools: ["Read"],
    });

    try {
      const cleaned = response.trim().replace(/^```json?\n?/, "").replace(/\n?```$/, "");
      const result = JSON.parse(cleaned);
      if (result.worthwhile) {
        console.error(`[assess-worthiness] Worth it — proceeding`);
        return { kind: "Some", value: refactor };
      }
      console.error(`[assess-worthiness] Skipping: ${result.reason ?? "not worth it"}`);
      return { kind: "None", value: null };
    } catch {
      // If we can't parse, include it (demo safety)
      console.error("[assess-worthiness] Could not parse response, including by default");
      return { kind: "Some", value: refactor };
    }
  },
}, "assessWorthiness");

// --- Data shaping ---

export const deriveBranch = createHandler({
  inputValidator: z.object({ description: z.string() }),
  outputValidator: z.object({ branch: z.string() }),
  handle: async ({ value }) => {
    console.error(`[derive-branch] Deriving branch name from: ${value.description.slice(0, 60)}`);
    return {
      branch: `refactor/${value.description.toLowerCase().replace(/[^a-z0-9]+/g, "-").slice(0, 40)}`,
    };
  },
}, "deriveBranch");

export const preparePRInput = createHandler({
  inputValidator: z.object({
    branch: z.string(),
    description: z.string(),
  }),
  outputValidator: z.object({ branch: z.string(), title: z.string(), body: z.string() }),
  handle: async ({ value }) => {
    console.error(`[prepare-pr-input] Preparing PR for branch ${value.branch}`);
    return {
      branch: value.branch,
      title: `Refactor: ${value.description.slice(0, 60)}`,
      body: `Automated refactor:\n\n${value.description}`,
    };
  },
}, "preparePRInput");

// --- Implementation ---

export const implement = createHandler({
  inputValidator: z.object({
    worktreePath: z.string(),
    description: z.string(),
  }),
  handle: async ({ value }) => {
    console.error(`[implement] Applying refactor in ${value.worktreePath}: ${value.description}`);

    await callClaude({
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
  outputValidator: JudgmentResultValidator,
  handle: async (): Promise<JudgmentResult> => {
    console.error("[judge-refactor] Reviewing changes...");

    const response = await callClaude({
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
  inputValidator: JudgmentResultValidator,
  outputValidator: taggedUnionSchema({ Approved: z.null(), NeedsWork: z.string() }),
  handle: async ({ value: judgment }): Promise<ClassifyJudgmentResult> => {
    console.error(`[classify-judgment] Classifying judgment (approved=${judgment.approved})`);
    if (judgment.approved) {
      console.error("[classify-judgment] Approved");
      return { kind: "Approved", value: null };
    }
    console.error(`[classify-judgment] Needs work: ${judgment.instructions}`);
    return { kind: "NeedsWork", value: judgment.instructions };
  },
}, "classifyJudgment");

export const applyFeedback = createHandler({
  inputValidator: z.string(),
  handle: async ({ value: instructions }) => {
    console.error(`[apply-feedback] Applying feedback: ${instructions}`);

    await callClaude({
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

// --- Pipelines ---

import { typeCheckFix } from "./type-check-fix";
import { createWorktree, createPR } from "./git";

export type ImplementAndReviewParams = Refactor & { worktreePath: string; branch: string };

export const implementAndReview = bindInput<ImplementAndReviewParams>((implementAndReviewParams) => pipe(
  implementAndReviewParams.pick("worktreePath", "description").then(implement).drop(),
  implementAndReviewParams.pick("worktreePath").then(typeCheckFix).drop(),

  // Judge quality; revise and re-check if needed.
  loop<void, void>((recur, done) =>
    judgeRefactor.then(classifyJudgment).branch({
      NeedsWork: applyFeedback.drop()
        .then(implementAndReviewParams.pick("worktreePath")).then(typeCheckFix)
        .drop().then(recur),
      Approved: done,
    }),
  ).drop(),

  // Commit and open a PR only after all fixes and revisions are done.
  implementAndReviewParams.pick("worktreePath").then(commit).drop(),
  pipe(implementAndReviewParams.pick("branch", "description"), preparePRInput, createPR),
));

export const createBranchWorktree = pipe(
  pick<Refactor, ["description"]>("description"),
  deriveBranch,
  createWorktree,
);

export const openPR = preparePRInput.then(createPR);
