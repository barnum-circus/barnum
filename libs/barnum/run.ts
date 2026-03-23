import { spawn, type ChildProcess } from "node:child_process";
import { chmodSync } from "node:fs";
import { createRequire } from "node:module";
import type { Cli, Command, ConfigCommand } from "./barnum-cli-schema.zod.js";

const require = createRequire(import.meta.url);
const binaryPath: string = require("./index.cjs");

function spawnBarnum(args: string[]): ChildProcess {
  try {
    chmodSync(binaryPath, 0o755);
  } catch {}
  return spawn(binaryPath, args, { stdio: "inherit" });
}

function camelToKebab(s: string): string {
  return s.replace(/[A-Z]/g, (m) => `-${m.toLowerCase()}`);
}

function pushFields(
  args: string[],
  obj: Record<string, unknown>,
  skip: string[],
): void {
  for (const [key, value] of Object.entries(obj)) {
    if (skip.includes(key) || value == null) continue;
    if (typeof value === "boolean") {
      if (value) args.push(`--${camelToKebab(key)}`);
    } else {
      args.push(`--${camelToKebab(key)}`, String(value));
    }
  }
}

function pushGlobalArgs(args: string[], cli: Partial<Cli>): void {
  if (cli.root) args.push("--root", cli.root);
  if (cli.logLevel) args.push("--log-level", cli.logLevel);
}

export function barnum(cli: Cli): ChildProcess {
  const args: string[] = [];
  pushGlobalArgs(args, cli);

  switch (cli.command.kind) {
    case "Run": {
      args.push("run");
      pushFields(args, cli.command, ["kind"]);
      return spawnBarnum(args);
    }
    case "Config": {
      args.push("config");
      const sub = cli.command.command;
      args.push(sub.kind.toLowerCase());
      pushFields(args, sub, ["kind"]);
      return spawnBarnum(args);
    }
    case "Version": {
      args.push("version");
      pushFields(args, cli.command, ["kind"]);
      return spawnBarnum(args);
    }
  }
}

type GlobalOpts = { root?: string; logLevel?: Cli["logLevel"] };

export function barnumRun(
  opts: Omit<Extract<Command, { kind: "Run" }>, "kind">,
  global?: GlobalOpts,
): ChildProcess {
  return barnum({
    ...global,
    command: { kind: "Run", ...opts },
  } as Cli);
}

export function barnumConfig(
  sub: ConfigCommand,
  global?: GlobalOpts,
): ChildProcess {
  return barnum({
    ...global,
    command: { kind: "Config", command: sub },
  } as Cli);
}

export function barnumVersion(
  opts?: Omit<Extract<Command, { kind: "Version" }>, "kind">,
  global?: GlobalOpts,
): ChildProcess {
  return barnum({
    ...global,
    command: { kind: "Version", json: false, ...opts },
  } as Cli);
}
