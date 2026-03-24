import { z } from "zod";

const ActionKind = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("Bash"),
    script: z.string().describe("Shell script to execute."),
  }).describe("Run a shell command."),
]).describe("How a resolved step processes tasks.");

const Options = z.object({
  maxRetries: z.number().int().nonnegative().optional().default(0).describe("Maximum retries."),
  retryOnInvalidResponse: z.boolean().optional().default(true).describe("Whether to retry on invalid response."),
  retryOnTimeout: z.boolean().optional().default(true).describe("Whether to retry on timeout."),
  timeout: z.number().int().nonnegative().nullable().optional().describe("Timeout in seconds."),
}).describe("Resolved options for a step.");

const Step = z.object({
  action: ActionKind.describe("How to execute the step."),
  finally: z.string().nullable().optional().describe("Finally hook (runs after all children complete)."),
  name: z.string().describe("Step name."),
  next: z.array(z.string()).optional().default([]).describe("Valid next steps."),
  options: Options.describe("Effective options (global + per-step merged)."),
}).describe("A fully resolved step.");

const Config = z.object({
  maxConcurrency: z.number().int().nonnegative().nullable().optional().describe("Maximum concurrent tasks (None = use default)."),
  steps: z.array(Step).describe("Resolved step definitions."),
}).describe("Fully resolved Barnum configuration.\n\nAll file references have been resolved and options computed per-step.");

const Task = z.object({
  kind: z.string().describe("The step name (serialized as \"kind\" for compatibility with agent responses)."),
  value: z.any().describe("The task payload."),
}).describe("A task with its kind (step name) and value.");

export const resolvedTypesSchema = z.object({
  config: Config.describe("The resolved configuration."),
  task: Task.describe("A task (agent response element)."),
}).describe("Root type for generating the resolved schema.\n\nGroups the resolved config and task types so `schema_for!` produces a single schema containing all resolved runtime types. This struct exists only for schema generation — it's never constructed at runtime.");

export type ResolvedTypes = z.infer<typeof resolvedTypesSchema>;
export type ActionKind = z.infer<typeof ActionKind>;
export type Options = z.infer<typeof Options>;
export type Step = z.infer<typeof Step>;
export type Config = z.infer<typeof Config>;
export type Task = z.infer<typeof Task>;
