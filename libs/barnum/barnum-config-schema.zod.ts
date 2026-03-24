import { z } from "zod";

const ActionKind = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("Bash"),
    script: z.string().describe("Shell script to execute.\n\n**Input (stdin):** JSON object: `{\"kind\": \"<step name>\", \"value\": <payload>}`. Use `jq '.value'` to extract the payload, or `jq -r '.value.fieldName'` for a specific field.\n\n**Output (stdout):** JSON array of follow-up tasks to spawn: `[{\"kind\": \"NextStep\", \"value\": {...}}, ...]`. Each `kind` must be a step name listed in this step's `next` array. Return `[]` to spawn no follow-ups."),
  }).describe("Run a shell command."),
  z.object({
    exportedAs: z.string().optional().default("default").describe("Named export to invoke from the handler module."),
    kind: z.literal("TypeScript"),
    path: z.string().describe("Path to the handler file (absolute — JS layer resolves before passing to Rust)."),
    stepConfig: z.any().optional().default(null).describe("Step configuration passed through to the handler. Rust stores this as-is and includes it in the envelope."),
  }).describe("Run a TypeScript handler."),
]).describe("How a step processes tasks.");

const FinallyHook = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("Bash"),
    script: z.string().describe("Shell script to execute."),
  }).describe("Run a shell command as the finally hook."),
]).describe("Finally hook. Runs after a task and all its descendants complete.\n\nIn JSON: `{\"kind\": \"Bash\", \"script\": \"./finally-hook.sh\"}`\n\n**stdin:** JSON object: `{\"kind\": \"<step name>\", \"value\": <payload>}`. **stdout:** JSON array of follow-up tasks: `[{\"kind\": \"StepName\", \"value\": {...}}, ...]`. Return `[]` for no follow-ups.");

const Options = z.object({
  maxConcurrency: z.number().int().nonnegative().nullable().optional().default(null).describe("Maximum concurrent tasks (None = unlimited)."),
  maxRetries: z.number().int().nonnegative().optional().default(0).describe("Maximum retries per task (default: 0)."),
  retryOnInvalidResponse: z.boolean().optional().default(true).describe("Whether to retry when agent returns invalid response (default: true)."),
  retryOnTimeout: z.boolean().optional().default(true).describe("Whether to retry when agent times out (default: true)."),
  timeout: z.number().int().nonnegative().nullable().optional().default(null).describe("Timeout in seconds for each task (None = no timeout)."),
}).strict().describe("Global runtime options for task execution. All fields have sensible defaults.");

const StepOptions = z.object({
  maxRetries: z.number().int().nonnegative().nullable().optional().default(null).describe("Maximum retries for tasks on this step (overrides global `max_retries`)."),
  retryOnInvalidResponse: z.boolean().nullable().optional().default(null).describe("Whether to retry when an agent returns an invalid response on this step (overrides global `retry_on_invalid_response`)."),
  retryOnTimeout: z.boolean().nullable().optional().default(null).describe("Whether to retry when an agent times out on this step (overrides global `retry_on_timeout`)."),
  timeout: z.number().int().nonnegative().nullable().optional().default(null).describe("Timeout in seconds for tasks on this step (overrides global `timeout`)."),
}).strict().describe("Per-step option overrides. Only set the fields you want to override; omitted fields inherit from the global `options`.");

const Step = z.object({
  action: ActionKind.describe("How this step processes tasks."),
  finally: FinallyHook.nullable().optional().default(null).describe("Shell script that runs after this task **and all tasks it spawned (recursively)** have completed.\n\n**stdin:** JSON object: `{\"kind\": \"<step name>\", \"value\": <payload>}`. Same envelope format as command action scripts.\n\n**stdout:** A JSON array of follow-up tasks to spawn: `[{\"kind\": \"StepName\", \"value\": {...}}, ...]`. Each `kind` must be a valid step name. Return `[]` to spawn no follow-ups.\n\nUse this for cleanup, aggregation, or spawning a final summarization step after an entire subtree of work completes."),
  name: z.string().describe("Unique name for this step (e.g., `\"Analyze\"`, `\"Implement\"`, `\"Review\"`). This is the string used as `kind` when creating tasks: `{\"kind\": \"ThisStepName\", \"value\": {...}}`."),
  next: z.array(z.string()).optional().default([]).describe("Step names this step is allowed to spawn follow-up tasks on. Each string must match the `name` of another step in this config. An empty array means this is a terminal step (no follow-ups)."),
  options: StepOptions.optional().default({"maxRetries": null, "retryOnInvalidResponse": null, "retryOnTimeout": null, "timeout": null}).describe("Per-step options that override the global `options`. Only the fields you set here take effect; everything else falls through to the global defaults."),
}).strict().describe("A named step in the workflow. Steps are the nodes of the task graph.\n\nThe `finally` hook runs after the task **and all of its descendant tasks** complete.");

export const configSchema = z.object({
  entrypoint: z.string().nullable().optional().default(null).describe("Name of the step that starts the workflow. When set, the CLI accepts `--entrypoint-value` to provide the initial task value (defaults to `{}`). When omitted, `--initial-state` must provide explicit `[{\"kind\": \"StepName\", \"value\": ...}]` tasks."),
  options: Options.optional().default({"maxConcurrency": null, "maxRetries": 0, "retryOnInvalidResponse": true, "retryOnTimeout": true, "timeout": null}).describe("Global runtime options (timeout, retries, concurrency). Individual steps can override these via their own `options` field."),
  steps: z.array(Step).describe("The steps that make up this workflow. Each step defines how to process a task and which steps it can spawn follow-up tasks on."),
}).strict().describe("Top-level Barnum configuration.\n\nDefines a workflow as a directed graph of steps. Each step processes tasks and can spawn follow-up tasks on other steps.");

export type Config = z.infer<typeof configSchema>;
export type ActionKind = z.infer<typeof ActionKind>;
export type FinallyHook = z.infer<typeof FinallyHook>;
export type Options = z.infer<typeof Options>;
export type StepOptions = z.infer<typeof StepOptions>;
export type Step = z.infer<typeof Step>;

export function defineConfig(config: z.input<typeof configSchema>): Config {
  return configSchema.parse(config);
}
