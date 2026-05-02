// callClaude — spawn Claude CLI in non-interactive mode.
//
// Configuration:
//   PREFIX_COMMAND controls how the Claude CLI is invoked.
//   - Set GSD_PREFIX env var to use a wrapper (e.g. GSD_PREFIX=ai-sandbox runs `ai-sandbox claude -p ...`)
//   - Without GSD_PREFIX, runs `claude -p ...` directly (no wrapper)

import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const PREFIX_COMMAND: string | null = process.env.GSD_PREFIX ?? null;

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const baseDir = path.resolve(__dirname, "..");

const CLAUDE_TIMEOUT_MS = 5 * 60_000; // 5 minutes

/** Spawn Claude CLI in non-interactive mode. Streams output to stderr, returns full stdout. */
export async function callClaude(args: {
  prompt: string;
  allowedTools?: string[];
  cwd?: string;
}): Promise<string> {
  const command = PREFIX_COMMAND ?? "claude";
  const cliArgs = [
    ...(PREFIX_COMMAND != null ? ["claude"] : []),
    "-p",
    args.prompt,
    "--output-format",
    "text",
    "--dangerously-skip-permissions",
  ];
  if (args.allowedTools && args.allowedTools.length > 0) {
    cliArgs.push("--allowedTools", ...args.allowedTools);
  }

  function shellQuote(arg: string): string {
    if (/[^a-zA-Z0-9_\-=/:.,@]/.test(arg)) {
      return `'${arg.replace(/'/g, "'\\''")}'`;
    }
    return arg;
  }
  console.error(
    `[callClaude] $ ${command} ${cliArgs.map(shellQuote).join(" ")}`,
  );

  return new Promise<string>((resolve, reject) => {
    let settled = false;

    const child = spawn(command, cliArgs, {
      cwd: args.cwd ?? baseDir,
      stdio: ["ignore", "pipe", "pipe"],
      env: {
        ...process.env,
        // Prevent "nested session" error if run from within Claude Code
        CLAUDECODE: undefined,
        CLAUDE_CODE_ENTRYPOINT: undefined,
      },
    });

    const timeout = setTimeout(() => {
      if (!settled) {
        settled = true;
        console.error(
          `[callClaude] timed out after ${CLAUDE_TIMEOUT_MS / 1000}s, killing`,
        );
        child.kill("SIGTERM");
        reject(
          new Error(`Claude CLI timed out after ${CLAUDE_TIMEOUT_MS / 1000}s`),
        );
      }
    }, CLAUDE_TIMEOUT_MS);

    const stdoutChunks: Buffer[] = [];

    child.stdout.on("data", (chunk: Buffer) => {
      stdoutChunks.push(chunk);
      process.stderr.write(chunk);
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
        console.error(`[callClaude] killed by signal ${signal}`);
        reject(new Error(`Claude CLI killed by ${signal}`));
        return;
      }
      if (code !== 0) {
        console.error(`[callClaude] exited with code ${code}`);
        reject(new Error(`Claude CLI exited with code ${code}`));
        return;
      }
      console.error(
        `[callClaude] completed successfully (${stdout.length} chars)`,
      );
      resolve(stdout);
    });
  });
}
