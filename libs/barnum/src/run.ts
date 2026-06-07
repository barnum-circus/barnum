/**
 * Workflow execution: resolves the barnum binary, tsx executor, and worker
 * script, then spawns the workflow as a subprocess.
 */

import { execFileSync, spawn as nodeSpawn } from "node:child_process";
import { createRequire } from "node:module";
import { existsSync, mkdtempSync, unlinkSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import type { Action, ExtractOutput } from "./ast.js";

/** Log verbosity for the barnum engine runtime. Passed to the CLI's `--log-level`. */
export type LogLevel = "off" | "error" | "warn" | "info" | "debug" | "trace";

export interface RunOptions {
  /** Engine log verbosity. Default: "off" (only handler stderr is visible). */
  readonly logLevel?: LogLevel;
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
  | { readonly kind: "Env"; readonly path: string }
  | { readonly kind: "NodeModules"; readonly path: string }
  | { readonly kind: "Local"; readonly path: string };

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

/** Build the barnum binary if using the local dev path. Skips if binary already exists. */
function buildBinaryIfNeeded(binaryPath: string): void {
  if (existsSync(binaryPath)) {
    return;
  }
  const repoRoot = path.resolve(__dirname, "../../..");
  // eslint-disable-next-line no-console
  console.error("[barnum] building CLI binary (cargo build -p barnum_cli)...");
  execFileSync("cargo", ["build", "-p", "barnum_cli"], {
    cwd: repoRoot,
    stdio: "inherit",
  });
}

/**
 * A compiled, runnable workflow. Owns the serialized config JSON and carries
 * the pipeline's output type as a phantom parameter so `run()` stays typed.
 *
 * Construct one with `pipeline.compile()` (from an in-memory AST) or
 * `CompiledWorkflow.fromJSON(configJson)` (from a raw config-JSON string).
 */
export class CompiledWorkflow<TPipeline extends Action> {
  /** The serialized config JSON — the compiled artifact, exposed directly. */
  readonly configJson: string;
  /** Phantom — carries ExtractOutput<TPipeline> so run() stays typed. */
  declare readonly __output?: ExtractOutput<TPipeline>;

  private constructor(configJson: string) {
    this.configJson = configJson;
  }

  /** Build from an in-memory pipeline AST. Walks the AST and serializes the config. */
  static fromAction<TAction extends Action>(
    action: TAction,
  ): CompiledWorkflow<TAction> {
    return new CompiledWorkflow<TAction>(JSON.stringify({ workflow: action }));
  }

  /**
   * Wrap an existing config-JSON string (e.g. read back from disk). No AST work.
   *
   * The output type can't be recovered from JSON, so `run()` resolves to
   * `unknown` — the expected tradeoff for the no-TypeScript escape hatch.
   */
  static fromJSON(configJson: string): CompiledWorkflow<Action> {
    return new CompiledWorkflow<Action>(configJson);
  }

  /** Run the compiled workflow to completion. Returns the final output value. */
  run(options?: RunOptions): Promise<ExtractOutput<TPipeline>> {
    return spawnBarnumJson(this.configJson, options?.logLevel);
  }
}

/** Spawn the barnum CLI with the given config JSON. Returns the parsed final value from stdout. */
function spawnBarnumJson<TOut>(
  configJson: string,
  logLevel?: LogLevel,
): Promise<TOut> {
  const binaryResolution = resolveBinary();
  if (binaryResolution.kind === "Local") {
    buildBinaryIfNeeded(binaryResolution.path);
  }
  const executor = resolveExecutor();
  const worker = resolveWorker();

  const tmpDir = mkdtempSync(path.join(os.tmpdir(), "barnum-"));
  const configFilePath = path.join(tmpDir, "config.json");
  writeFileSync(configFilePath, configJson);

  const cliArgs = [
    "run",
    "--config-file",
    configFilePath,
    "--executor",
    executor,
    "--worker",
    worker,
  ];
  if (logLevel) {
    cliArgs.push("--log-level", logLevel);
  }

  return new Promise<TOut>((resolve, reject) => {
    // NOT detached: barnum makes ITSELF a process-group leader (setpgid(0,0) in
    // barnum_cli) and installs its own SIGINT/SIGTERM handler that killpg()s the
    // group — barnum plus the `tsx worker.ts` children it spawns. Spawning
    // detached here would call setpgid in the child too and collide with that
    // (EPERM), so we leave grouping to barnum.
    const child = nodeSpawn(binaryResolution.path, cliArgs, {
      stdio: ["inherit", "pipe", "pipe"],
    });

    const stdoutChunks: Array<Buffer> = [];
    const stderrChunks: Array<Buffer> = [];

    child.stdout?.on("data", (chunk: Buffer) => {
      stdoutChunks.push(chunk);
    });

    child.stderr?.on("data", (chunk: Buffer) => {
      stderrChunks.push(chunk);
      process.stderr.write(chunk);
    });

    // Forward a parent termination signal to barnum, then remove our handlers
    // and re-raise on ourselves so the parent exits with the conventional
    // signal status. An interactive Ctrl+C already reaches barnum via the
    // terminal's foreground group, but a programmatic kill of the parent (e.g.
    // a supervisor, or `/loop`) does NOT — without this forward, barnum (and
    // its workers, possibly mid-`git` holding an index.lock) would orphan.
    // barnum's own signal handler then killpg()s its worker group.
    const forwardAndExit = (signal: NodeJS.Signals) => {
      if (child.pid !== undefined) {
        try {
          process.kill(child.pid, signal);
        } catch {
          // Already exited, or we lost the race — nothing to do.
        }
      }
      process.off("SIGINT", onSigint);
      process.off("SIGTERM", onSigterm);
      process.kill(process.pid, signal);
    };
    const onSigint = () => forwardAndExit("SIGINT");
    const onSigterm = () => forwardAndExit("SIGTERM");
    process.on("SIGINT", onSigint);
    process.on("SIGTERM", onSigterm);

    const cleanup = () => {
      process.off("SIGINT", onSigint);
      process.off("SIGTERM", onSigterm);
      try {
        unlinkSync(configFilePath);
      } catch {
        /* best-effort */
      }
    };

    child.on("error", (error) => {
      cleanup();
      reject(new Error(`Failed to spawn barnum: ${error.message}`));
    });

    child.on("close", (code) => {
      cleanup();
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
