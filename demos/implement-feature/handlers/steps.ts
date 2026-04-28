/**
 * Handlers for the implement-feature demo.
 *
 * setup: clear out/, copy src/ to out/
 * implement: invoke Claude to implement the feature
 * reviewBestPractices / reviewAdherence / checkSuppressedTests: review handlers
 * runTypecheck: run tsc --noEmit on out/
 * classifyFeedback: combine check results into HasIssues / AllClean
 * incorporateFeedback: invoke Claude to address issues
 * splitCommits: no-op (always clean)
 * checkRetries: decrement retry budget for withRetry
 */

import {
  createHandler,
  ok,
  err,
  resultSchema,
  taggedUnionSchema,
} from "@barnum/barnum/runtime";
import type { Result, TaggedUnion } from "@barnum/barnum/pipeline";
import {
  cpSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
} from "node:fs";
import { spawnSync } from "node:child_process";
import path from "node:path";
import { z } from "zod";
import { baseDir, srcDir, outDir } from "./lib";
import { callClaude } from "./call-claude";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

type CheckResult = Result<string[], string>;
type ActionResult = Result<void, string>;

const CheckResultValidator = resultSchema(z.array(z.string()), z.string());
const ActionResultValidator = resultSchema(z.void(), z.string());

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

export const setup = createHandler(
  {
    handle: async (): Promise<void> => {
      console.error("[setup] Clearing output directory...");
      if (existsSync(outDir)) {
        rmSync(outDir, { recursive: true });
      }
      mkdirSync(outDir, { recursive: true });

      console.error(`[setup] Copying ${srcDir} → ${outDir}`);
      cpSync(srcDir, outDir, { recursive: true });

      const files = readdirSync(outDir);
      console.error(
        `[setup] Copied ${files.length} files: ${files.join(", ")}`,
      );
    },
  },
  "setup",
);

// ---------------------------------------------------------------------------
// Implement
// ---------------------------------------------------------------------------

export const implement = createHandler(
  {
    inputValidator: z.string(),
    outputValidator: ActionResultValidator,
    handle: async ({ value: description }): Promise<ActionResult> => {
      console.error(`[implement] Task: ${description}`);

      try {
        await callClaude({
          prompt: [
            `Implement this feature in the codebase at ${outDir}:`,
            "",
            description,
            "",
            "The codebase already has a fetchSuggestions function in autocomplete.ts.",
            "Use it — do not reimplement autocomplete logic.",
            "Add debouncing (300ms) so the API is not called on every keystroke.",
            "Show suggestions in a dropdown list below the input.",
            "Update the tests to cover the new behavior.",
          ].join("\n"),
          allowedTools: [
            `Read(//${outDir}/**)`,
            `Edit(//${outDir}/**)`,
            `Write(//${outDir}/**)`,
          ],
          cwd: outDir,
        });
        console.error("[implement] Feature implemented");
        return ok(undefined);
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        console.error(`[implement] Failed: ${msg}`);
        return err(msg);
      }
    },
  },
  "implement",
);

// ---------------------------------------------------------------------------
// Review: best practices
// ---------------------------------------------------------------------------

export const reviewBestPractices = createHandler(
  {
    outputValidator: CheckResultValidator,
    handle: async (): Promise<CheckResult> => {
      console.error("[reviewBestPractices] Reviewing...");

      const files = readdirSync(outDir)
        .filter((f) => f.endsWith(".tsx"))
        .map((f) => path.join(outDir, f));

      try {
        const response = await callClaude({
          prompt: [
            "Review these React files for best practices violations.",
            "Check for:",
            "- Missing cleanup of effects (useEffect without cleanup)",
            "- Inline function definitions in JSX that should use useCallback",
            "- Missing key props in lists",
            "- Direct state mutation",
            "- Missing error boundaries for async operations",
            "",
            "Files to review:",
            ...files.map((f) => `  ${f}`),
            "",
            "Respond with ONLY a JSON array of issue strings.",
            "If no issues: []",
            'If issues: ["issue 1 description", "issue 2 description"]',
          ].join("\n"),
          allowedTools: [`Read(//${outDir}/**)`],
          cwd: outDir,
        });

        const issues = parseJsonArray(response);
        console.error(
          `[reviewBestPractices] ${issues.length === 0 ? "Clean" : `${issues.length} issue(s)`}`,
        );
        return ok(issues);
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        console.error(`[reviewBestPractices] Failed: ${msg}`);
        return err(msg);
      }
    },
  },
  "reviewBestPractices",
);

// ---------------------------------------------------------------------------
// Review: adherence to task
// ---------------------------------------------------------------------------

export const reviewAdherence = createHandler(
  {
    inputValidator: z.string(),
    outputValidator: CheckResultValidator,
    handle: async ({ value: description }): Promise<CheckResult> => {
      console.error("[reviewAdherence] Checking adherence to task...");

      const files = readdirSync(outDir)
        .filter((f) => f.endsWith(".ts") || f.endsWith(".tsx"))
        .map((f) => path.join(outDir, f));

      try {
        const response = await callClaude({
          prompt: [
            "Check whether the implementation adheres to this task description:",
            "",
            description,
            "",
            "Files to review:",
            ...files.map((f) => `  ${f}`),
            "",
            "Specifically check:",
            "- Does it use the existing fetchSuggestions function (not a reimplementation)?",
            "- Is the input debounced (not firing on every keystroke)?",
            "- Are suggestions displayed to the user?",
            "",
            "Respond with ONLY a JSON array of issue strings.",
            "If fully adherent: []",
            'If issues: ["issue 1", "issue 2"]',
          ].join("\n"),
          allowedTools: [`Read(//${outDir}/**)`],
          cwd: outDir,
        });

        const issues = parseJsonArray(response);
        console.error(
          `[reviewAdherence] ${issues.length === 0 ? "Clean" : `${issues.length} issue(s)`}`,
        );
        return ok(issues);
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        console.error(`[reviewAdherence] Failed: ${msg}`);
        return err(msg);
      }
    },
  },
  "reviewAdherence",
);

// ---------------------------------------------------------------------------
// Review: suppressed tests
// ---------------------------------------------------------------------------

export const checkSuppressedTests = createHandler(
  {
    outputValidator: CheckResultValidator,
    handle: async (): Promise<CheckResult> => {
      console.error("[checkSuppressedTests] Scanning for suppressed tests...");

      const testFiles = readdirSync(outDir).filter(
        (f) => f.includes(".test.") || f.includes(".spec."),
      );

      const issues: string[] = [];
      const suppressionPatterns = [
        { pattern: /\bit\.skip\b/, label: "it.skip" },
        { pattern: /\btest\.skip\b/, label: "test.skip" },
        { pattern: /\bdescribe\.skip\b/, label: "describe.skip" },
        { pattern: /\bxit\b/, label: "xit" },
        { pattern: /\bxdescribe\b/, label: "xdescribe" },
        { pattern: /\bxtest\b/, label: "xtest" },
      ];

      for (const file of testFiles) {
        const content = readFileSync(path.join(outDir, file), "utf-8");
        for (const { pattern, label } of suppressionPatterns) {
          if (pattern.test(content)) {
            issues.push(`${file}: contains ${label} — suppressed test`);
          }
        }
      }

      console.error(
        `[checkSuppressedTests] ${issues.length === 0 ? "Clean" : `${issues.length} suppressed test(s)`}`,
      );
      return ok(issues);
    },
  },
  "checkSuppressedTests",
);

// ---------------------------------------------------------------------------
// Typecheck
// ---------------------------------------------------------------------------

export const runTypecheck = createHandler(
  {
    outputValidator: CheckResultValidator,
    handle: async (): Promise<CheckResult> => {
      console.error(`[runTypecheck] Running tsc --noEmit on ${outDir}...`);

      const tsFiles = readdirSync(outDir)
        .filter((f) => f.endsWith(".ts") || f.endsWith(".tsx"))
        .map((f) => path.join(outDir, f));

      if (tsFiles.length === 0) {
        console.error("[runTypecheck] No TS files found");
        return ok([]);
      }

      const tscPath = path.join(baseDir, "node_modules", ".bin", "tsc");
      const result = spawnSync(
        tscPath,
        [
          "--noEmit",
          "--strict",
          "--esModuleInterop",
          "--target",
          "ES2020",
          "--module",
          "ES2020",
          "--moduleResolution",
          "node",
          "--jsx",
          "react-jsx",
          ...tsFiles,
        ],
        { encoding: "utf-8", cwd: baseDir, timeout: 30_000 },
      );

      const output = result.stdout + result.stderr;
      const issues = parseTscErrors(output);

      console.error(
        `[runTypecheck] ${issues.length === 0 ? "Clean" : `${issues.length} error(s)`}`,
      );
      return ok(issues);
    },
  },
  "runTypecheck",
);

// ---------------------------------------------------------------------------
// Classify feedback
// ---------------------------------------------------------------------------

type ClassifyFeedbackResultDef = {
  HasIssues: string;
  AllClean: void;
};
export type ClassifyFeedbackResult = TaggedUnion<
  "ClassifyFeedbackResult",
  ClassifyFeedbackResultDef
>;

const FeedbackInputValidator = z.object({
  bestPractices: z.array(z.string()),
  adherence: z.array(z.string()),
  suppressedTests: z.array(z.string()),
  typecheck: z.array(z.string()),
});

export const classifyFeedback = createHandler(
  {
    inputValidator: FeedbackInputValidator,
    outputValidator: taggedUnionSchema("ClassifyFeedbackResult", {
      HasIssues: z.string(),
      AllClean: z.null(),
    }),
    handle: async ({ value }): Promise<ClassifyFeedbackResult> => {
      const allIssues: string[] = [
        ...value.bestPractices,
        ...value.adherence,
        ...value.suppressedTests,
        ...value.typecheck,
      ];

      if (allIssues.length > 0) {
        const summary = allIssues.join("; ");
        console.error(
          `[classifyFeedback] ${allIssues.length} issue(s): ${summary.slice(0, 120)}`,
        );
        return { kind: "ClassifyFeedbackResult.HasIssues", value: summary };
      }

      console.error("[classifyFeedback] All checks passed");
      return { kind: "ClassifyFeedbackResult.AllClean", value: null };
    },
  },
  "classifyFeedback",
);

// ---------------------------------------------------------------------------
// Incorporate feedback
// ---------------------------------------------------------------------------

export const incorporateFeedback = createHandler(
  {
    inputValidator: z.string(),
    outputValidator: ActionResultValidator,
    handle: async ({ value: feedback }): Promise<ActionResult> => {
      console.error(
        `[incorporateFeedback] Addressing: ${feedback.slice(0, 120)}...`,
      );

      try {
        await callClaude({
          prompt: [
            `Fix the following issues in the codebase at ${outDir}:`,
            "",
            feedback,
            "",
            "Make minimal changes. Do not change behavior beyond what's needed to fix the issues.",
            "Do not suppress or skip any tests.",
          ].join("\n"),
          allowedTools: [
            `Read(//${outDir}/**)`,
            `Edit(//${outDir}/**)`,
            `Write(//${outDir}/**)`,
          ],
          cwd: outDir,
        });
        console.error("[incorporateFeedback] Feedback incorporated");
        return ok(undefined);
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        console.error(`[incorporateFeedback] Failed: ${msg}`);
        return err(msg);
      }
    },
  },
  "incorporateFeedback",
);

// ---------------------------------------------------------------------------
// Split commits (no-op)
// ---------------------------------------------------------------------------

export const splitCommits = createHandler(
  {
    outputValidator: ActionResultValidator,
    handle: async (): Promise<ActionResult> => {
      console.error("[splitCommits] No-op — skipping commit split");
      return ok(undefined);
    },
  },
  "splitCommits",
);

// ---------------------------------------------------------------------------
// Retry budget (used by withRetry)
// ---------------------------------------------------------------------------

type CheckRetriesResultDef = {
  Retry: number;
  Exhausted: void;
};
export type CheckRetriesResult = TaggedUnion<
  "CheckRetriesResult",
  CheckRetriesResultDef
>;

export const checkRetries = createHandler(
  {
    inputValidator: z.number(),
    outputValidator: taggedUnionSchema("CheckRetriesResult", {
      Retry: z.number(),
      Exhausted: z.null(),
    }),
    handle: async ({ value: remaining }): Promise<CheckRetriesResult> => {
      if (remaining > 0) {
        return { kind: "CheckRetriesResult.Retry", value: remaining - 1 };
      }
      return { kind: "CheckRetriesResult.Exhausted", value: null };
    },
  },
  "checkRetries",
);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Parse a JSON array from Claude's response, tolerating markdown fences. */
function parseJsonArray(response: string): string[] {
  const stripped = response
    .replace(/^```(?:json)?\n?/m, "")
    .replace(/\n?```$/m, "")
    .trim();
  try {
    const parsed: unknown = JSON.parse(stripped);
    if (Array.isArray(parsed) && parsed.every((x) => typeof x === "string")) {
      return parsed;
    }
  } catch {
    // Claude didn't return valid JSON — treat as single issue
  }
  if (stripped && stripped !== "[]") {
    return [stripped];
  }
  return [];
}

/** Parse tsc error output into issue strings. */
function parseTscErrors(output: string): string[] {
  const issues: string[] = [];
  const pattern = /^(.+?)\((\d+),(\d+)\): error (TS\d+): (.+)$/gm;
  let match;
  while ((match = pattern.exec(output)) !== null) {
    issues.push(`${match[1]}(${match[2]}): ${match[4]}: ${match[5]}`);
  }
  return issues;
}
