// Shared utilities for demo handlers.

import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
export const baseDir = path.resolve(__dirname, "..");

const CLAUDE_TIMEOUT_MS = 5 * 60_000; // 5 minutes

/** Send a one-off prompt to Claude via ai-sandbox CLI. Returns the text response. */
export async function callClaude(prompt: string): Promise<string> {
  const cliArgs = [
    "claude",
    "-p",
    prompt,
    "--output-format",
    "text",
    "--dangerously-skip-permissions",
  ];

  console.error(`[callClaude] Sending prompt (${prompt.length} chars)...`);

  return new Promise<string>((resolve, reject) => {
    let settled = false;

    const child = spawn("ai-sandbox", cliArgs, {
      cwd: baseDir,
      stdio: ["ignore", "pipe", "pipe"],
      env: {
        ...process.env,
        CLAUDECODE: undefined,
        CLAUDE_CODE_ENTRYPOINT: undefined,
      },
    });

    const timeout = setTimeout(() => {
      if (!settled) {
        settled = true;
        child.kill("SIGTERM");
        reject(new Error(`Claude CLI timed out after ${CLAUDE_TIMEOUT_MS / 1000}s`));
      }
    }, CLAUDE_TIMEOUT_MS);

    const stdoutChunks: Buffer[] = [];

    child.stdout.on("data", (chunk: Buffer) => {
      stdoutChunks.push(chunk);
    });

    child.stderr.on("data", (chunk: Buffer) => {
      process.stderr.write(chunk);
    });

    child.on("error", (error) => {
      if (!settled) {
        settled = true;
        clearTimeout(timeout);
        reject(new Error(`Claude CLI failed: ${error.message}`));
      }
    });

    child.on("close", (code, signal) => {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);

      const stdout = Buffer.concat(stdoutChunks).toString("utf-8");
      if (signal) {
        reject(new Error(`Claude CLI killed by ${signal}`));
        return;
      }
      if (code !== 0) {
        reject(new Error(`Claude CLI exited with code ${code}`));
        return;
      }
      console.error(`[callClaude] Received response (${stdout.length} chars)`);
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
