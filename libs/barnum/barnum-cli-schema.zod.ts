import { z } from "zod";

const SchemaType = z.union([
  z.literal("zod").describe("Zod TypeScript schema."),
  z.literal("json").describe("JSON Schema."),
]).describe("Output format for `barnum config schema`.");

const ConfigCommand = z.discriminatedUnion("kind", [
  z.object({
    config: z.string().describe("Config (JSON string or path to file)"),
    kind: z.literal("Docs"),
  }).describe("Generate markdown documentation from config"),
  z.object({
    config: z.string().describe("Config (JSON string or path to file)"),
    kind: z.literal("Validate"),
  }).describe("Validate a config file"),
  z.object({
    config: z.string().describe("Config (JSON string or path to file)"),
    kind: z.literal("Graph"),
  }).describe("Generate DOT visualization of config (for `GraphViz`)"),
  z.object({
    kind: z.literal("Schema"),
    schemaType: SchemaType.describe("Output format: zod (default) or json"),
  }).describe("Print the config schema (Zod by default, JSON with --type json)"),
]).describe("Subcommands for `barnum config`.");

const Command = z.discriminatedUnion("kind", [
  z.object({
    config: z.string().nullable().optional().describe("Config (JSON string or path to file). Required unless `--resume-from` is used."),
    entrypointValue: z.string().nullable().optional().describe("Initial value for the entrypoint step (JSON string or path to file). Only valid when config has an `entrypoint`. Defaults to `{}` if not provided."),
    executor: z.string().nullable().optional().describe("Internal: executor command injected by cli.cjs. Not user-facing — hidden from --help."),
    initialState: z.string().nullable().optional().describe("Initial tasks (JSON string or path to file). Required if config has no `entrypoint`. Cannot be used with `--entrypoint-value`."),
    kind: z.literal("Run"),
    logFile: z.string().nullable().optional().describe("Log file path (logs emitted in addition to stderr)"),
    resumeFrom: z.string().nullable().optional().describe("Resume from a previous state log file. Incompatible with `--config`, `--initial-state`, and `--entrypoint-value`."),
    stateLog: z.string().nullable().optional().describe("State log file path (NDJSON file for persistence/resume)"),
    wake: z.string().nullable().optional().describe("Wake script to call before starting"),
  }).describe("Run the task queue"),
  z.object({
    command: ConfigCommand.describe("Config subcommand to run."),
    kind: z.literal("Config"),
  }).describe("Config file operations (docs, validate, graph, schema)"),
  z.object({
    json: z.boolean().describe("Output as JSON (for programmatic access)"),
    kind: z.literal("Version"),
  }).describe("Print version information"),
]).describe("Barnum subcommands.");

const LogLevel = z.union([
  z.literal("off").describe("No logging"),
  z.literal("error").describe("Error messages only"),
  z.literal("warn").describe("Warnings and errors"),
  z.literal("info").describe("Informational messages (default)"),
  z.literal("debug").describe("Debug messages (includes task return values)"),
  z.literal("trace").describe("Trace messages (very verbose)"),
]).describe("Log level for barnum output.");

export const cliSchema = z.object({
  command: Command.describe("Subcommand to run."),
  logLevel: LogLevel.describe("Log level (debug shows task return values)"),
}).describe("Top-level CLI arguments for barnum.");

export type Cli = z.infer<typeof cliSchema>;
export type SchemaType = z.infer<typeof SchemaType>;
export type ConfigCommand = z.infer<typeof ConfigCommand>;
export type Command = z.infer<typeof Command>;
export type LogLevel = z.infer<typeof LogLevel>;
