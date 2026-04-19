/**
 * Vitest global setup: builds the barnum binary once before any tests run.
 * Without this, each test file triggers a concurrent `cargo build` and they
 * serialize on the cargo file lock, causing timeouts.
 */

import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import path from "node:path";

export function setup(): void {
  const repoRoot = path.resolve(import.meta.dirname, "../../..");
  if (!existsSync(path.join(repoRoot, "Cargo.toml"))) {
    return; // Not in local dev — binary comes from env or node_modules
  }
  // eslint-disable-next-line no-console
  console.error(
    "[vitest global-setup] building CLI binary (cargo build -p barnum_cli)...",
  );
  execFileSync("cargo", ["build", "-p", "barnum_cli"], {
    cwd: repoRoot,
    stdio: "inherit",
  });
}
