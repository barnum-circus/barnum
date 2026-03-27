import { execFileSync } from "child_process";
import { fileURLToPath } from "url";
import path from "path";
import { describe, expect, it } from "vitest";

import {
  all,
  attempt,
  loop,
  matchCases,
  sequence,
  traverse,
  type Config,
} from "../src/core.js";
import setup from "./handlers/setup.js";
import process_ from "./handlers/process.js";
import check from "./handlers/check.js";
import finalize from "./handlers/finalize.js";
import validate from "./handlers/validate.js";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const BINARY = path.resolve(HERE, "../../../target/debug/barnum");

/** Pipe JSON through `barnum run` and parse the output. */
function roundTrip(input: unknown): unknown {
  const json = JSON.stringify(input);
  const stdout = execFileSync(BINARY, ["run"], {
    input: json,
    encoding: "utf-8",
  });
  return JSON.parse(stdout);
}

/**
 * Round-trip tests verify JSON serialization, not type safety.
 * Config objects are constructed directly to avoid the never-input
 * constraint of config(), which is tested separately in types.test.ts.
 */
describe("barnum round-trip", () => {
  it("Call", () => {
    const cfg: Config = { workflow: setup() };
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Sequence", () => {
    const cfg: Config = { workflow: sequence(setup(), process_()) };
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("All", () => {
    const cfg: Config = { workflow: all(check(), check()) };
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Traverse", () => {
    const cfg: Config = { workflow: traverse(check()) };
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Attempt", () => {
    const cfg: Config = { workflow: attempt(check()) };
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Match", () => {
    const cfg: Config = {
      workflow: matchCases({ yes: finalize(), no: finalize() }),
    };
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Loop", () => {
    const cfg: Config = { workflow: loop(validate()) };
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Step", () => {
    const steps: Config["steps"] = { DoCheck: check() };
    const cfg: Config = {
      workflow: { kind: "Step", step: "DoCheck" },
      steps,
    };
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("combined workflow", () => {
    const steps: Config["steps"] = { Recheck: check() };
    const cfg: Config = {
      workflow: sequence(
        setup(),
        process_(),
        attempt(check()),
        matchCases({
          Ok: finalize(),
          Err: finalize(),
        }),
      ),
      steps,
    };
    expect(roundTrip(cfg)).toEqual(cfg);
  });
});
