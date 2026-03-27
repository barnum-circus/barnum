import { execFileSync } from "child_process";
import { fileURLToPath } from "url";
import path from "path";
import { describe, expect, it } from "vitest";

import {
  all,
  attempt,
  call,
  config,
  configBuilder,
  loop,
  matchCases,
  sequence,
  traverse,
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

describe("barnum round-trip", () => {
  it("Call", () => {
    const cfg = config(call(setup));
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Sequence", () => {
    const cfg = config(sequence(call(setup), call(process_)));
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("All", () => {
    const cfg = config(all(call(check), call(check)));
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Traverse", () => {
    const cfg = config(traverse(call(check)));
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Attempt", () => {
    const cfg = config(attempt(call(check)));
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Match", () => {
    const cfg = config(
      matchCases({ yes: call(finalize), no: call(finalize) }),
    );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Loop", () => {
    const cfg = config(loop(call(validate)));
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Step", () => {
    const cfg = configBuilder()
      .registerSteps({ DoCheck: call(check) })
      .workflow((steps) => steps.DoCheck);
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("combined workflow", () => {
    const cfg = configBuilder()
      .registerSteps({ Recheck: call(check) })
      .workflow((steps) =>
        sequence(
          call(setup),
          call(process_),
          call(check),
          matchCases({
            pass: call(finalize),
            fail: call(finalize),
          }),
        ),
      );
    expect(roundTrip(cfg)).toEqual(cfg);
  });
});
