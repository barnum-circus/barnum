/**
 * Workflow output demo: verifies pipeline.run() returns the final value.
 *
 * Runs three pipelines and asserts the returned values match expectations.
 * Demonstrates that run() captures the Rust CLI's stdout as a typed
 * return value instead of printing it to the terminal.
 *
 * Usage: pnpm exec tsx run.ts
 */

import { constant } from "@barnum/barnum/pipeline";
import { double, addLabel } from "./handlers/steps";

async function main() {
  // 1. Constant value round-trips through the engine
  const number = await constant(42).run();
  console.error(`[assert] constant(42) returned: ${JSON.stringify(number)}`);
  assert(number === 42, `expected 42, got ${JSON.stringify(number)}`);

  // 2. Handler output is captured
  const doubled = await double.call(constant(21)).run();
  console.error(`[assert] double(21) returned: ${JSON.stringify(doubled)}`);
  assert(doubled === 42, `expected 42, got ${JSON.stringify(doubled)}`);

  // 3. Multi-step pipeline returns the final handler's output
  const fiveDoubled = double.call(constant(5));
  const labeled = await addLabel.call(fiveDoubled).run();
  console.error(
    `[assert] addLabel(double(5)) returned: ${JSON.stringify(labeled)}`,
  );
  assert(
    labeled.label === "result-10" && labeled.value === 10,
    `expected {label: "result-10", value: 10}, got ${JSON.stringify(labeled)}`,
  );

  console.error("\nAll assertions passed.");
}

function assert(condition: boolean, message: string): asserts condition {
  if (!condition) {
    throw new Error(`Assertion failed: ${message}`);
  }
}

main();
