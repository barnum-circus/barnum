/**
 * Workflow execution: resolves the barnum binary, tsx executor, and worker
 * script, then spawns the workflow as a subprocess.
 */

import { execFileSync, spawn as nodeSpawn } from "node:child_process";
import { createRequire } from "node:module";
import { existsSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import type { Config } from "./ast.js";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

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

  if (platform === "darwin" && arch === "arm64") artifactDir = "macos-arm64";
  else if (platform === "darwin") artifactDir = "macos-x64";
  else if (platform === "linux" && arch === "arm64") artifactDir = "linux-arm64";
  else if (platform === "linux") artifactDir = "linux-x64";
  else if (platform === "win32") { artifactDir = "win-x64"; binaryName = "barnum.exe"; }
  else return undefined;

  const callerRequire = createRequire(process.argv[1] || import.meta.url);
  try {
    const packageDir = path.dirname(callerRequire.resolve("@barnum/barnum/package.json"));
    const binaryPath = path.join(packageDir, "artifacts", artifactDir, binaryName);
    if (existsSync(binaryPath)) return binaryPath;
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

/** Run a workflow config to completion. Prints result to stdout. */
export async function run(config: Config): Promise<void> {
  const binaryResolution = resolveBinary();
  if (binaryResolution.kind === "Local") {
    buildBinary();
  }
  const executor = resolveExecutor();
  const worker = resolveWorker();
  const configJson = JSON.stringify(config);

  return new Promise<void>((resolve, reject) => {
    const child = nodeSpawn(binaryResolution.path, [
      "run",
      "--config", configJson,
      "--executor", executor,
      "--worker", worker,
    ], {
      stdio: ["inherit", "inherit", "pipe"],
    });

    const stderrChunks: Buffer[] = [];

    child.stderr!.on("data", (chunk: Buffer) => {
      stderrChunks.push(chunk);
      process.stderr.write(chunk);
    });

    child.on("error", (error) => {
      reject(new Error(`Failed to spawn barnum: ${error.message}`));
    });

    child.on("close", (code) => {
      if (code !== 0) {
        const stderr = Buffer.concat(stderrChunks).toString("utf-8").trim();
        const message = stderr
          ? `barnum exited with code ${code}:\n${stderr}`
          : `barnum exited with code ${code} (no stderr output)`;
        reject(new Error(message));
        return;
      }
      resolve();
    });
  });
}
