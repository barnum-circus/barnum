// Shared utilities for demo handlers.

import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
export const baseDir = path.resolve(__dirname, "..");

/** Spawn Claude CLI in non-interactive mode. Streams output to stderr, returns full stdout. */
export async function callClaude(args: {
  prompt: string;
  allowedTools?: string[];
  cwd?: string;
}): Promise<string> {
  const cliArgs = [
    "-p",
    args.prompt,
    "--output-format",
    "text",
    "--dangerously-skip-permissions",
  ];
  if (args.allowedTools && args.allowedTools.length > 0) {
    cliArgs.push("--allowedTools", ...args.allowedTools);
  }

  console.error(`[callClaude] $ claude ${cliArgs.map(a => a.includes(" ") ? JSON.stringify(a) : a).join(" ")}`);

  return new Promise<string>((resolve, reject) => {
    const child = spawn("claude", cliArgs, {
      cwd: args.cwd ?? baseDir,
      env: {
        ...process.env,
        // Prevent "nested session" error if run from within Claude Code
        CLAUDECODE: undefined,
        CLAUDE_CODE_ENTRYPOINT: undefined,
      },
    });

    const stdoutChunks: Buffer[] = [];

    child.stdout.on("data", (chunk: Buffer) => {
      stdoutChunks.push(chunk);
      process.stderr.write(chunk);
    });

    child.stderr.on("data", (chunk: Buffer) => {
      process.stderr.write(chunk);
    });

    child.on("error", (error) => {
      reject(new Error(`Claude CLI failed: ${error.message}`));
    });

    child.on("close", (code) => {
      const stdout = Buffer.concat(stdoutChunks).toString("utf-8");
      if (code !== 0) {
        reject(new Error(`Claude CLI exited with code ${code}`));
        return;
      }
      resolve(stdout);
    });
  });
}

/** Strip markdown code fences if present. */
export function stripCodeFences(text: string): string {
  const fenced = text.match(/^```(?:typescript|ts)?\n([\s\S]*?)\n```$/m);
  if (fenced) {
    return fenced[1];
  }
  return text.trim();
}
