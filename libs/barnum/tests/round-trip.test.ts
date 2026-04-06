import { execFileSync } from "child_process";
import { existsSync } from "fs";
import { fileURLToPath } from "url";
import path from "path";
import { beforeEach, describe, expect, it } from "vitest";

import {
  all,
  config,
  loop,
  branch,
  pipe,
  forEach,
  resetEffectIdCounter,
} from "../src/ast.js";
import { constant } from "../src/builtins.js";
import {
  setup,
  build,
  verify,
  deploy,
  healthCheck,
} from "./handlers.js";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const BINARY = path.resolve(HERE, "../../../target/debug/barnum");
const HAS_BINARY = existsSync(BINARY);

/** Pipe JSON through `barnum check --config` and parse the output. */
function roundTrip(input: unknown): unknown {
  const json = JSON.stringify(input);
  const stdout = execFileSync(BINARY, ["check", "--config", json], {
    encoding: "utf-8",
  });
  return JSON.parse(stdout);
}

describe.skipIf(!HAS_BINARY)("barnum round-trip", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("Invoke", () => {
    const cfg = config(
      pipe(constant({ project: "test" }), setup),
    );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Pipe", () => {
    const cfg = config(
      pipe(constant({ project: "test" }), setup, build),
    );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("All", () => {
    const cfg = config(
      pipe(constant({ artifact: "test" }), all(verify, verify)),
    );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("ForEach", () => {
    const cfg = config(
      pipe(constant([{ artifact: "test" }]), forEach(verify)),
    );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Branch", () => {
    type BranchIn =
      | { kind: "Yes"; value: { verified: boolean } }
      | { kind: "No"; value: { verified: boolean } };
    const cfg = config(
      pipe(
        constant<BranchIn>({ kind: "Yes", value: { verified: true } }),
        branch({ Yes: deploy, No: deploy }),
      ),
    );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Loop", () => {
    const cfg = config(
      constant({ deployed: true }).then(
        loop<{ stable: true }, { deployed: boolean }>((recur, done) =>
          healthCheck.branch({ Continue: recur, Break: done }),
        ),
      ),
    );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

});
