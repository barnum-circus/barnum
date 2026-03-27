import { execFileSync } from "child_process";
import { fileURLToPath } from "url";
import path from "path";
import { describe, expect, it } from "vitest";

import {
  all,
  attempt,
  call,
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
  it("exercises every action kind through the Rust binary", () => {
    const cfg = configBuilder()
      .registerSteps({ Finalize: call(finalize) })
      .workflow((steps) =>
        sequence(
          call(setup),
          all(call(process_), call(check)),
          traverse(call(finalize)),
          attempt(call(check)),
          matchCases({
            Success: steps.Finalize,
            Failure: call(setup),
          }),
          loop(sequence(call(check), call(finalize))),
        ),
      );

    const output = roundTrip(cfg);
    expect(output).toEqual(cfg);
  });
});
