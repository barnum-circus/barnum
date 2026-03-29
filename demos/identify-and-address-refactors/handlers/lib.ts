// Shared utilities for demo handlers.

import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
export const baseDir = path.resolve(__dirname, "..");

/** Spawn Claude CLI in non-interactive mode. Returns stdout. */
export function callClaude(args: {
  prompt: string;
  allowedTools?: string[];
  cwd?: string;
}): string {
  const cliArgs = [
    "-p", args.prompt,
    "--output-format", "text",
    "--dangerously-skip-permissions",
  ];
  if (args.allowedTools && args.allowedTools.length > 0) {
    cliArgs.push("--allowedTools", ...args.allowedTools);
  }

  const result = spawnSync("claude", cliArgs, {
    encoding: "utf-8",
    cwd: args.cwd ?? baseDir,
    timeout: 300_000,
    env: {
      ...process.env,
      // Prevent "nested session" error if run from within Claude Code
      CLAUDECODE: undefined,
      CLAUDE_CODE_ENTRYPOINT: undefined,
    },
  });

  if (result.error) {
    throw new Error(`Claude CLI failed: ${result.error.message}`);
  }
  if (result.status !== 0) {
    throw new Error(`Claude CLI exited with code ${result.status}: ${result.stderr}`);
  }

  return result.stdout;
}

/** Strip markdown code fences if present. */
export function stripCodeFences(text: string): string {
  const fenced = text.match(/^```(?:typescript|ts)?\n([\s\S]*?)\n```$/m);
  if (fenced) {
    return fenced[1];
  }
  return text.trim();
}
