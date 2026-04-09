/**
 * Workflow execution: resolves the barnum binary, tsx executor, and worker
 * script, then spawns the workflow as a subprocess.
 */

import { execFileSync, spawn as nodeSpawn } from "node:child_process";
import { createRequire } from "node:module";
import { existsSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import type { Action, Config, ExtractOutput, Pipeable } from "./ast.js";
import { chain } from "./chain.js";
import { constant } from "./builtins.js";

/** Log verbosity for the barnum engine runtime. Passed to the CLI's `--log-level`. */
export type LogLevel = "off" | "error" | "warn" | "info" | "debug" | "trace";

export interface RunPipelineOptions {
  /** Engine log verbosity. Default: "off" (only handler stderr is visible). */
  logLevel?: LogLevel;
}

const __dirname = import.meta.dirname;

/** Resolve the TypeScript executor. Uses bun if the workflow was launched with bun, otherwise tsx. */
function resolveExecutor(): string {
  if (process.versions.bun) {
    return "bun";
  }
  const callerRequire = createRequire(process.argv[1] || import.meta.url);
  const tsxPath = callerRequire.resolve("tsx/cli");
  return `node ${tsxPath}`;
}

/** Resolve the platform-specific binary from the @barnum/barnum package artifacts. */
function resolveInstalledBinary(): string | undefined {
  const platform = os.platform();
  const arch = os.arch();

  let artifactDir: string;
  let binaryName = "barnum";

  if (platform === "darwin" && arch === "arm64") {
    artifactDir = "macos-arm64";
  } else if (platform === "darwin") {
    artifactDir = "macos-x64";
  } else if (platform === "linux" && arch === "arm64") {
    artifactDir = "linux-arm64";
  } else if (platform === "linux") {
    artifactDir = "linux-x64";
  } else if (platform === "win32") {
    artifactDir = "win-x64";
    binaryName = "barnum.exe";
  } else {
    return undefined;
  }

  const callerRequire = createRequire(process.argv[1] || import.meta.url);
  try {
    const packageDir = path.dirname(
      callerRequire.resolve("@barnum/barnum/package.json"),
    );
    const binaryPath = path.join(
      packageDir,
      "artifacts",
      artifactDir,
      binaryName,
    );
    if (existsSync(binaryPath)) {
      return binaryPath;
    }
  } catch {
    // Package not installed
  }
  return undefined;
}

type BinaryResolution =
  | { kind: "Env"; path: string }
  | { kind: "NodeModules"; path: string }
  | { kind: "Local"; path: string };

/** Resolve the barnum binary. Checks: BARNUM env var, local repo, node_modules. */
function resolveBinary(): BinaryResolution {
  if (process.env.BARNUM) {
    return { kind: "Env", path: process.env.BARNUM };
  }

  const repoRoot = path.resolve(__dirname, "../../..");
  if (existsSync(path.join(repoRoot, "Cargo.toml"))) {
    return {
      kind: "Local",
      path: path.join(repoRoot, "target/debug/barnum"),
    };
  }

  const installedBinaryPath = resolveInstalledBinary();
  if (installedBinaryPath) {
    return { kind: "NodeModules", path: installedBinaryPath };
  }

  throw new Error(
    "Could not find barnum binary. Set BARNUM env var or install @barnum/barnum.",
  );
}

/** Resolve worker.ts relative to this package. */
function resolveWorker(): string {
  return path.resolve(__dirname, "../src/worker.ts");
}

/** Build the barnum binary if using the local dev path. */
function buildBinary(): void {
  const repoRoot = path.resolve(__dirname, "../../..");
  execFileSync("cargo", ["build", "-p", "barnum_cli"], {
    cwd: repoRoot,
    stdio: "ignore",
  });
}

/** Run a pipeline to completion. Returns the workflow's final output value. */
export function runPipeline<TPipeline extends Action>(
  pipeline: TPipeline,
  input?: unknown,
  options?: RunPipelineOptions,
): Promise<ExtractOutput<TPipeline>> {
  const workflow =
    input === undefined
      ? pipeline
      : (chain(constant(input) as Pipeable, pipeline as Pipeable) as Action);
  return spawnBarnum({ workflow }, options?.logLevel);
}

/** Spawn the barnum CLI with the given config. Returns the parsed final value from stdout. */
function spawnBarnum<TOut>(config: Config, logLevel?: LogLevel): Promise<TOut> {
  const binaryResolution = resolveBinary();
  if (binaryResolution.kind === "Local") {
    buildBinary();
  }
  const executor = resolveExecutor();
  const worker = resolveWorker();
  const configJson = JSON.stringify(config);

  const cliArgs = [
    "run",
    "--config",
    configJson,
    "--executor",
    executor,
    "--worker",
    worker,
  ];
  if (logLevel) {
    cliArgs.push("--log-level", logLevel);
  }

  return new Promise<TOut>((resolve, reject) => {
    const child = nodeSpawn(binaryResolution.path, cliArgs, {
      stdio: ["inherit", "pipe", "pipe"],
    });

    const stdoutChunks: Buffer[] = [];
    const stderrChunks: Buffer[] = [];

    child.stdout?.on("data", (chunk: Buffer) => {
      stdoutChunks.push(chunk);
    });

    child.stderr?.on("data", (chunk: Buffer) => {
      stderrChunks.push(chunk);
      process.stderr.write(chunk);
    });

    child.on("error", (error) => {
      reject(new Error(`Failed to spawn barnum: ${error.message}`));
    });

    child.on("close", (code) => {
      if (code !== 0) {
        const stderr = Buffer.concat(stderrChunks).toString("utf8").trim();
        const detail = stderr ? `\n${stderr}` : "";
        reject(new Error(`barnum exited with code ${code}${detail}`));
        return;
      }
      const stdout = Buffer.concat(stdoutChunks).toString("utf8").trim();
      if (!stdout) {
        resolve(undefined as TOut);
        return;
      }
      try {
        resolve(JSON.parse(stdout) as TOut);
      } catch {
        reject(
          new Error(`barnum produced non-JSON output on stdout: ${stdout}`),
        );
      }
    });
  });
}
