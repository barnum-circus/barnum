import { execFileSync } from "child_process";
import { fileURLToPath } from "url";
import path from "path";
import { describe, expect, it } from "vitest";

import {
  parallel,
  attempt,
  configBuilder,
  loop,
  branch,
  pipe,
  forEach,
} from "../src/ast.js";
import { constant } from "../src/builtins.js";
import {
  setup,
  process,
  check,
  finalize,
  validate,
} from "./handlers.js";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const BINARY = path.resolve(HERE, "../../../target/debug/barnum");

/** Pipe JSON through `barnum run --config` and parse the output. */
function roundTrip(input: unknown): unknown {
  const json = JSON.stringify(input);
  const stdout = execFileSync(BINARY, ["run", "--config", json], {
    encoding: "utf-8",
  });
  return JSON.parse(stdout);
}

describe("barnum round-trip", () => {
  it("Invoke", () => {
    const cfg = configBuilder().workflow(() =>
      pipe(constant({ project: "test" }), setup()),
    );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Pipe", () => {
    const cfg = configBuilder().workflow(() =>
      pipe(constant({ project: "test" }), setup(), process()),
    );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Parallel", () => {
    const cfg = configBuilder().workflow(() =>
      pipe(constant({ result: "test" }), parallel(check(), check())),
    );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("ForEach", () => {
    const cfg = configBuilder().workflow(() =>
      pipe(constant([{ result: "test" }]), forEach(check())),
    );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Attempt", () => {
    const cfg = configBuilder().workflow(() =>
      pipe(constant({ result: "test" }), attempt(check())),
    );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Branch", () => {
    const cfg = configBuilder().workflow(() =>
      pipe(
        constant({ kind: "Yes" }),
        branch({ Yes: finalize(), No: finalize() }),
      ),
    );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Loop", () => {
    const cfg = configBuilder().workflow(() =>
      pipe(constant({ valid: true }), loop(validate())),
    );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Step", () => {
    const cfg = configBuilder()
      .registerSteps({ DoCheck: check() })
      .workflow(({ steps }) =>
        pipe(constant({ result: "test" }), steps.DoCheck),
      );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("combined workflow", () => {
    const cfg = configBuilder()
      .registerSteps({ Recheck: check() })
      .workflow(({ steps }) =>
        pipe(
          constant({ project: "test" }),
          setup(),
          process(),
          attempt(steps.Recheck),
          branch({
            Ok: finalize(),
            Err: finalize(),
          }),
        ),
      );
    expect(roundTrip(cfg)).toEqual(cfg);
  });
});
