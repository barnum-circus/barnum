import { execFileSync } from "child_process";
import { fileURLToPath } from "url";
import path from "path";
import { describe, expect, it } from "vitest";

import {
  parallel,
  workflowBuilder,
  loop,
  branch,
  pipe,
  forEach,
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

/** Pipe JSON through `barnum check --config` and parse the output. */
function roundTrip(input: unknown): unknown {
  const json = JSON.stringify(input);
  const stdout = execFileSync(BINARY, ["check", "--config", json], {
    encoding: "utf-8",
  });
  return JSON.parse(stdout);
}

describe("barnum round-trip", () => {
  it("Invoke", () => {
    const cfg = workflowBuilder().workflow(() =>
      pipe(constant({ project: "test" }), setup),
    );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Pipe", () => {
    const cfg = workflowBuilder().workflow(() =>
      pipe(constant({ project: "test" }), setup, build),
    );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Parallel", () => {
    const cfg = workflowBuilder().workflow(() =>
      pipe(constant({ artifact: "test" }), parallel(verify, verify)),
    );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("ForEach", () => {
    const cfg = workflowBuilder().workflow(() =>
      pipe(constant([{ artifact: "test" }]), forEach(verify)),
    );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Branch", () => {
    const cfg = workflowBuilder().workflow(() =>
      pipe(
        constant({ kind: "Yes" } as const),
        branch({ Yes: deploy, No: deploy }),
      ),
    );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Loop", () => {
    const cfg = workflowBuilder().workflow(() =>
      pipe(constant({ deployed: true }), loop(healthCheck)),
    );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("Step", () => {
    const cfg = workflowBuilder()
      .registerSteps({ DoVerify: verify })
      .workflow(({ steps }) =>
        pipe(constant({ artifact: "test" }), steps.DoVerify),
      );
    expect(roundTrip(cfg)).toEqual(cfg);
  });

  it("combined workflow", () => {
    const cfg = workflowBuilder()
      .registerSteps({ Recheck: verify })
      .workflow(({ steps }) =>
        pipe(
          constant({ project: "test" }),
          setup,
          build,
          steps.Recheck,
          deploy,
        ),
      );
    expect(roundTrip(cfg)).toEqual(cfg);
  });
});
