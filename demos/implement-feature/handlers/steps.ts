/**
 * Mock handlers for the implement-feature demo.
 *
 * Each review/check handler returns Result<string[], string>:
 *   Ok([])           — no issues found
 *   Ok(["issue..."])  — issues found (not an error — normal outcome)
 *   Err("msg")        — transient failure (network, timeout, etc.)
 *
 * The implement/incorporate handlers return Result<void, string>:
 *   Ok(void)  — success
 *   Err("msg") — transient failure
 */

import { createHandler, ok, err, resultSchema } from "@barnum/barnum/runtime";
import type { Result } from "@barnum/barnum/runtime";
import { taggedUnionSchema } from "@barnum/barnum/runtime";
import type { TaggedUnion } from "@barnum/barnum/pipeline";
import { z } from "zod";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

type CheckResult = Result<string[], string>;
type ActionResult = Result<void, string>;

const CheckResultValidator = resultSchema(z.array(z.string()), z.string());
const ActionResultValidator = resultSchema(z.void(), z.string());

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Simulate a transient failure ~20% of the time. */
function maybeTransientFailure(stepName: string): string | null {
  if (Math.random() < 0.2) {
    return `${stepName}: transient failure (network timeout)`;
  }
  return null;
}

// ---------------------------------------------------------------------------
// Implementation handlers
// ---------------------------------------------------------------------------

/** Implement the feature based on the description. */
export const implement = createHandler(
  {
    inputValidator: z.string(),
    outputValidator: ActionResultValidator,
    handle: async ({ value: description }): Promise<ActionResult> => {
      const failure = maybeTransientFailure("implement");
      if (failure) {
        console.error(`[implement] ${failure}`);
        return err(failure);
      }

      console.error(`[implement] Implementing: ${description.slice(0, 60)}...`);
      console.error("[implement] Feature implemented");
      return ok(undefined);
    },
  },
  "implement",
);

/** Incorporate review feedback into the implementation. */
export const incorporateFeedback = createHandler(
  {
    inputValidator: z.string(),
    outputValidator: ActionResultValidator,
    handle: async ({ value: feedback }): Promise<ActionResult> => {
      const failure = maybeTransientFailure("incorporateFeedback");
      if (failure) {
        console.error(`[incorporateFeedback] ${failure}`);
        return err(failure);
      }

      console.error(
        `[incorporateFeedback] Addressing: ${feedback.slice(0, 80)}...`,
      );
      console.error("[incorporateFeedback] Feedback incorporated");
      return ok(undefined);
    },
  },
  "incorporateFeedback",
);

// ---------------------------------------------------------------------------
// Review handlers (agent-based, hardcoded criteria)
// ---------------------------------------------------------------------------

const securityIssues = [
  "SQL injection risk in query builder",
  "Unsanitized user input in template rendering",
  "Hardcoded API key in config module",
];

export const reviewSecurity = createHandler(
  {
    outputValidator: CheckResultValidator,
    handle: async (): Promise<CheckResult> => {
      const failure = maybeTransientFailure("reviewSecurity");
      if (failure) {
        console.error(`[reviewSecurity] ${failure}`);
        return err(failure);
      }

      console.error("[reviewSecurity] Reviewing for security issues...");
      const issues =
        Math.random() < 0.4
          ? [securityIssues[Math.floor(Math.random() * securityIssues.length)]!]
          : [];
      console.error(
        `[reviewSecurity] ${issues.length === 0 ? "Clean" : `Found: ${issues[0]}`}`,
      );
      return ok(issues);
    },
  },
  "reviewSecurity",
);

const qualityIssues = [
  "Function exceeds 50 lines — extract helper",
  "Duplicated logic between handleCreate and handleUpdate",
  "Missing error handling in async pipeline",
];

export const reviewQuality = createHandler(
  {
    outputValidator: CheckResultValidator,
    handle: async (): Promise<CheckResult> => {
      const failure = maybeTransientFailure("reviewQuality");
      if (failure) {
        console.error(`[reviewQuality] ${failure}`);
        return err(failure);
      }

      console.error("[reviewQuality] Reviewing code quality...");
      const issues =
        Math.random() < 0.3
          ? [qualityIssues[Math.floor(Math.random() * qualityIssues.length)]!]
          : [];
      console.error(
        `[reviewQuality] ${issues.length === 0 ? "Clean" : `Found: ${issues[0]}`}`,
      );
      return ok(issues);
    },
  },
  "reviewQuality",
);

const adherenceIssues = [
  "Feature description says 'cache all endpoints' but only GET is cached",
  "Missing retry logic specified in requirements",
];

export const reviewAdherence = createHandler(
  {
    outputValidator: CheckResultValidator,
    handle: async (): Promise<CheckResult> => {
      const failure = maybeTransientFailure("reviewAdherence");
      if (failure) {
        console.error(`[reviewAdherence] ${failure}`);
        return err(failure);
      }

      console.error("[reviewAdherence] Checking adherence to spec...");
      const issues =
        Math.random() < 0.3
          ? [
              adherenceIssues[
                Math.floor(Math.random() * adherenceIssues.length)
              ]!,
            ]
          : [];
      console.error(
        `[reviewAdherence] ${issues.length === 0 ? "Clean" : `Found: ${issues[0]}`}`,
      );
      return ok(issues);
    },
  },
  "reviewAdherence",
);

// ---------------------------------------------------------------------------
// Static analysis handlers (deterministic tools)
// ---------------------------------------------------------------------------

export const runTypecheck = createHandler(
  {
    outputValidator: CheckResultValidator,
    handle: async (): Promise<CheckResult> => {
      const failure = maybeTransientFailure("runTypecheck");
      if (failure) {
        console.error(`[runTypecheck] ${failure}`);
        return err(failure);
      }

      console.error("[runTypecheck] Running tsc --noEmit...");
      const issues =
        Math.random() < 0.3
          ? ["TS2345: Argument of type 'string' is not assignable to 'number'"]
          : [];
      console.error(
        `[runTypecheck] ${issues.length === 0 ? "Clean" : `${issues.length} error(s)`}`,
      );
      return ok(issues);
    },
  },
  "runTypecheck",
);

export const runLint = createHandler(
  {
    outputValidator: CheckResultValidator,
    handle: async (): Promise<CheckResult> => {
      const failure = maybeTransientFailure("runLint");
      if (failure) {
        console.error(`[runLint] ${failure}`);
        return err(failure);
      }

      console.error("[runLint] Running linter...");
      const issues =
        Math.random() < 0.2
          ? ["no-unused-vars: 'tempResult' is defined but never used"]
          : [];
      console.error(
        `[runLint] ${issues.length === 0 ? "Clean" : `${issues.length} warning(s)`}`,
      );
      return ok(issues);
    },
  },
  "runLint",
);

export const runTests = createHandler(
  {
    outputValidator: CheckResultValidator,
    handle: async (): Promise<CheckResult> => {
      const failure = maybeTransientFailure("runTests");
      if (failure) {
        console.error(`[runTests] ${failure}`);
        return err(failure);
      }

      console.error("[runTests] Running test suite...");
      const issues =
        Math.random() < 0.25
          ? ["FAIL cache.test.ts: expected 200, got 404"]
          : [];
      console.error(
        `[runTests] ${issues.length === 0 ? "All passed" : `${issues.length} failure(s)`}`,
      );
      return ok(issues);
    },
  },
  "runTests",
);

// ---------------------------------------------------------------------------
// Retry budget
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
  security: z.array(z.string()),
  quality: z.array(z.string()),
  adherence: z.array(z.string()),
  typecheck: z.array(z.string()),
  lint: z.array(z.string()),
  tests: z.array(z.string()),
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
        ...value.security,
        ...value.quality,
        ...value.adherence,
        ...value.typecheck,
        ...value.lint,
        ...value.tests,
      ];

      if (allIssues.length > 0) {
        const summary = allIssues.join("; ");
        console.error(
          `[classifyFeedback] ${allIssues.length} issue(s): ${summary.slice(0, 100)}`,
        );
        return {
          kind: "ClassifyFeedbackResult.HasIssues",
          value: summary,
        };
      }

      console.error("[classifyFeedback] All checks passed");
      return { kind: "ClassifyFeedbackResult.AllClean", value: null };
    },
  },
  "classifyFeedback",
);
