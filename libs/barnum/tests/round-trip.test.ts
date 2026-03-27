import { execFileSync } from "child_process";
import { fileURLToPath } from "url";
import path from "path";
import { describe, expect, it } from "vitest";

import {
  all,
  attempt,
  configBuilder,
  loop,
  matchCases,
  sequence,
  traverse,
} from "../src/ast.js";
import { constant } from "../src/builtins.js";
import {
  setup,
  process_,
  check,
  finalize,
  validate,
} from "./handlers.js";

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
    const cfg = configBuilder().workflow(() =>
      sequence(constant({ project: "test" }), setup()),
    );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Sequence", () => {
    const cfg = configBuilder().workflow(() =>
      sequence(constant({ project: "test" }), setup(), process_()),
    );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("All", () => {
    const cfg = configBuilder().workflow(() =>
      sequence(constant({ result: "test" }), all(check(), check())),
    );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Traverse", () => {
    const cfg = configBuilder().workflow(() =>
      sequence(constant([{ result: "test" }]), traverse(check())),
    );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Attempt", () => {
    const cfg = configBuilder().workflow(() =>
      sequence(constant({ result: "test" }), attempt(check())),
    );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Match", () => {
    const cfg = configBuilder().workflow(() =>
      sequence(
        constant({ kind: "Yes" }),
        matchCases({ Yes: finalize(), No: finalize() }),
      ),
    );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Loop", () => {
    const cfg = configBuilder().workflow(() =>
      sequence(constant({ valid: true }), loop(validate())),
    );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Step", () => {
    const cfg = configBuilder()
      .registerSteps({ DoCheck: check() })
      .workflow((steps) =>
        sequence(constant({ result: "test" }), steps.DoCheck),
      );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("combined workflow", () => {
    const cfg = configBuilder()
      .registerSteps({ Recheck: check() })
      .workflow((steps) =>
        sequence(
          constant({ project: "test" }),
          setup(),
          process_(),
          attempt(steps.Recheck),
          matchCases({
            Ok: finalize(),
            Err: finalize(),
          }),
        ),
      );
    expect(roundTrip(cfg)).toEqual(cfg);
  });
});
