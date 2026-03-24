import { z } from "zod";

const MaybeLinked_for_String = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("Inline"),
    value: z.string().describe("The content value, provided directly in the config file."),
  }).describe("Inline content."),
  z.object({
    kind: z.literal("Link"),
    path: z.string().describe("Relative path to the file (resolved relative to the config file's directory)."),
  }).describe("Link to a file whose contents will be loaded at runtime."),
]).describe("Content that can be inline or linked to a file.\n\nIn config files: - `{\"kind\": \"Inline\", \"value\": <content>}` → content provided directly in the config - `{\"kind\": \"Link\", \"path\": \"file.md\"}` → content loaded from a file (path relative to the config file)");

const PoolActionFile = z.object({
  instructions: MaybeLinked_for_String.describe("Markdown prompt shown to the agent processing this task. This is the core of what tells the agent what to do. Use `{\"kind\": \"Inline\", \"value\": \"...\"}` to write the markdown directly, or `{\"kind\": \"Link\", \"path\": \"path/to/file.md\"}` to reference an external file."),
  pool: z.string().nullable().optional().default(null).describe("Pool name (e.g., `\"demo\"`, `\"reviewers\"`). If omitted, the pool infrastructure uses its own default."),
  root: z.string().nullable().optional().default(null).describe("Pool root directory. If omitted, the pool infrastructure uses its own default."),
  timeout: z.number().int().nonnegative().nullable().optional().default(null).describe("Agent timeout in seconds. Passed to the pool as `timeout_seconds` in the task payload. Controls how long the agent gets to work. Separate from the step-level `timeout` which controls barnum's worker timeout."),
}).describe("Send the task to the agent pool. An AI agent receives the task's `value` along with the `instructions` (markdown prompt) and produces a JSON array of follow-up tasks.");

const CommandActionFile = z.object({
  script: z.string().describe("Shell script to execute.\n\n**Input (stdin):** JSON object: `{\"kind\": \"<step name>\", \"value\": <payload>}`. Use `jq '.value'` to extract the payload, or `jq -r '.value.fieldName'` for a specific field.\n\n**Output (stdout):** JSON array of follow-up tasks to spawn: `[{\"kind\": \"NextStep\", \"value\": {...}}, ...]`. Each `kind` must be a step name listed in this step's `next` array. Return `[]` to spawn no follow-ups."),
}).describe("Run a local shell command instead of sending to an agent. Use this for deterministic transformations, fan-out, or glue logic.");

const ActionFile = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("Pool"),
    params: PoolActionFile,
  }).describe("Send the task to the agent pool for processing."),
  z.object({
    kind: z.literal("Command"),
    params: CommandActionFile,
  }).describe("Run a local shell command."),
]).describe("How a step processes tasks. Set `\"kind\": \"Pool\"` to send tasks to AI agents, or `\"kind\": \"Command\"` to run a local shell script.");

const HookCommand = z.object({
  script: z.string().describe("Shell script to execute."),
}).describe("A shell command used as a hook.");

const FinallyHook = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("Command"),
    params: HookCommand,
  }).describe("Run a shell command as the finally hook."),
]).describe("Finally hook. Runs after a task and all its descendants complete.\n\nIn JSON: `{\"kind\": \"Command\", \"params\": {\"script\": \"./finally-hook.sh\"}}`\n\n**stdin:** JSON object: `{\"kind\": \"<step name>\", \"value\": <payload>}`. **stdout:** JSON array of follow-up tasks: `[{\"kind\": \"StepName\", \"value\": {...}}, ...]`. Return `[]` for no follow-ups.");

const Options = z.object({
  max_concurrency: z.number().int().nonnegative().nullable().optional().default(null).describe("Maximum concurrent tasks (None = unlimited)."),
  max_retries: z.number().int().nonnegative().optional().default(0).describe("Maximum retries per task (default: 0)."),
  retry_on_invalid_response: z.boolean().optional().default(true).describe("Whether to retry when agent returns invalid response (default: true)."),
  retry_on_timeout: z.boolean().optional().default(true).describe("Whether to retry when agent times out (default: true)."),
  timeout: z.number().int().nonnegative().nullable().optional().default(null).describe("Timeout in seconds for each task (None = no timeout)."),
}).strict().describe("Global runtime options for task execution. All fields have sensible defaults.");

const StepOptions = z.object({
  max_retries: z.number().int().nonnegative().nullable().optional().default(null).describe("Maximum retries for tasks on this step (overrides global `max_retries`)."),
  retry_on_invalid_response: z.boolean().nullable().optional().default(null).describe("Whether to retry when an agent returns an invalid response on this step (overrides global `retry_on_invalid_response`)."),
  retry_on_timeout: z.boolean().nullable().optional().default(null).describe("Whether to retry when an agent times out on this step (overrides global `retry_on_timeout`)."),
  timeout: z.number().int().nonnegative().nullable().optional().default(null).describe("Timeout in seconds for tasks on this step (overrides global `timeout`)."),
}).strict().describe("Per-step option overrides. Only set the fields you want to override; omitted fields inherit from the global `options`.");

const StepFile = z.object({
  action: ActionFile.describe("How this step processes tasks — either send to the agent pool (`Pool`) or run a local shell command (`Command`)."),
  finally: FinallyHook.nullable().optional().default(null).describe("Shell script that runs after this task **and all tasks it spawned (recursively)** have completed.\n\n**stdin:** JSON object: `{\"kind\": \"<step name>\", \"value\": <payload>}`. Same envelope format as command action scripts.\n\n**stdout:** A JSON array of follow-up tasks to spawn: `[{\"kind\": \"StepName\", \"value\": {...}}, ...]`. Each `kind` must be a valid step name. Return `[]` to spawn no follow-ups.\n\nUse this for cleanup, aggregation, or spawning a final summarization step after an entire subtree of work completes."),
  name: z.string().describe("Unique name for this step (e.g., `\"Analyze\"`, `\"Implement\"`, `\"Review\"`). This is the string used as `kind` when creating tasks: `{\"kind\": \"ThisStepName\", \"value\": {...}}`."),
  next: z.array(z.string()).optional().default([]).describe("Step names this step is allowed to spawn follow-up tasks on. Each string must match the `name` of another step in this config. An empty array means this is a terminal step (no follow-ups)."),
  options: StepOptions.optional().default({"max_retries": null, "retry_on_invalid_response": null, "retry_on_timeout": null, "timeout": null}).describe("Per-step options that override the global `options`. Only the fields you set here take effect; everything else falls through to the global defaults."),
}).strict().describe("A named step in the workflow. Steps are the nodes of the task graph.\n\nThe `finally` hook runs after the task **and all of its descendant tasks** complete.");

export const configFileSchema = z.object({
  "$schema": z.string().nullable().optional().describe("Optional JSON Schema URL for editor validation (e.g., `\"./node_modules/@barnum/barnum/barnum-config-schema.json\"`). Ignored at runtime."),
  entrypoint: z.string().nullable().optional().default(null).describe("Name of the step that starts the workflow. When set, the CLI accepts `--entrypoint-value` to provide the initial task value (defaults to `{}`). When omitted, `--initial-state` must provide explicit `[{\"kind\": \"StepName\", \"value\": ...}]` tasks."),
  options: Options.optional().default({"max_concurrency": null, "max_retries": 0, "retry_on_invalid_response": true, "retry_on_timeout": true, "timeout": null}).describe("Global runtime options (timeout, retries, concurrency). Individual steps can override these via their own `options` field."),
  steps: z.array(StepFile).describe("The steps that make up this workflow. Each step defines how to process a task and which steps it can spawn follow-up tasks on."),
}).strict().describe("Top-level Barnum configuration file format.\n\nDefines a workflow as a directed graph of steps. Each step processes tasks and can spawn follow-up tasks on other steps.");

export type ConfigFile = z.infer<typeof configFileSchema>;
export type MaybeLinked_for_String = z.infer<typeof MaybeLinked_for_String>;
export type PoolActionFile = z.infer<typeof PoolActionFile>;
export type CommandActionFile = z.infer<typeof CommandActionFile>;
export type ActionFile = z.infer<typeof ActionFile>;
export type HookCommand = z.infer<typeof HookCommand>;
export type FinallyHook = z.infer<typeof FinallyHook>;
export type Options = z.infer<typeof Options>;
export type StepOptions = z.infer<typeof StepOptions>;
export type StepFile = z.infer<typeof StepFile>;

export function defineConfig(config: z.input<typeof configFileSchema>): ConfigFile {
  return configFileSchema.parse(config);
}
