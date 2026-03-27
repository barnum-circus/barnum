import { execFileSync } from "child_process";
import path from "path";
import { describe, expect, it } from "vitest";

import {
  all,
  attempt,
  call,
  config,
  loop,
  matchCases,
  sequence,
  step,
  traverse,
  typescript,
} from "./core.js";

const BINARY = path.resolve(__dirname, "../../../target/debug/barnum");

/** Pipe JSON through `barnum run` and parse the output. */
function roundTrip(input: unknown): unknown {
  const json = JSON.stringify(input);
  const stdout = execFileSync(BINARY, ["run"], { input: json, encoding: "utf-8" });
  return JSON.parse(stdout);
}

describe("barnum round-trip", () => {
  it("round-trips a full workflow through the Rust binary", () => {
    const workflow = config(
      sequence(
        call(typescript("./setup.ts", "setup")),
        all(
          call(typescript("./list.ts", "listFiles")),
          call(typescript("./ident.ts", "identity")),
        ),
        traverse(call(typescript("./migrate.ts", "migrate"))),
        attempt(call(typescript("./risky.ts", "tryIt"))),
        matchCases({
          Success: step("Process"),
          Failure: call(typescript("./fail.ts", "handleError")),
        }),
        loop(
          sequence(
            call(typescript("./check.ts", "typeCheck")),
            call(typescript("./signal.ts", "loopSignal")),
          ),
        ),
      ),
      { Process: call(typescript("./process.ts", "run")) },
    );

    const output = roundTrip(workflow);
    expect(output).toEqual(workflow);
  });
});
