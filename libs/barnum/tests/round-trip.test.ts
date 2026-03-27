import { execFileSync } from "child_process";
import { fileURLToPath } from "url";
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
} from "../src/core.js";

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
    const workflow = config(
      sequence(
        call(typescript("./setup.ts", "setup")),
        all(
          call(typescript("./a.ts", "taskA")),
          call(typescript("./b.ts", "taskB")),
        ),
        traverse(call(typescript("./each.ts", "process"))),
        attempt(call(typescript("./risky.ts", "tryIt"))),
        matchCases({
          Success: step("Finalize"),
          Failure: call(typescript("./fail.ts", "handleError")),
        }),
        loop(
          sequence(
            call(typescript("./check.ts", "poll")),
            call(typescript("./signal.ts", "decide")),
          ),
        ),
      ),
      { Finalize: call(typescript("./done.ts", "finalize")) },
    );

    const output = roundTrip(workflow);
    expect(output).toEqual(workflow);
  });
});
