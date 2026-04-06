import { existsSync } from "fs";
import { fileURLToPath } from "url";
import path from "path";
import { describe, it, expect } from "vitest";

import { pipe } from "../src/ast.js";
import { constant, identity } from "../src/builtins.js";
import { runPipeline } from "../src/run.js";
import { setup, build } from "./handlers.js";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const BINARY = path.resolve(HERE, "../../../target/debug/barnum");
const HAS_BINARY = existsSync(BINARY);

describe.skipIf(!HAS_BINARY)("runPipeline return value", () => {
  it("returns a number from constant", async () => {
    const result = await runPipeline(constant(42));
    expect(result).toBe(42);
  });

  it("returns a string from constant", async () => {
    const result = await runPipeline(constant("hello"));
    expect(result).toBe("hello");
  });

  it("returns an object from constant", async () => {
    const result = await runPipeline(constant({ x: 1, y: [2, 3] }));
    expect(result).toEqual({ x: 1, y: [2, 3] });
  });

  it("returns null from constant(null)", async () => {
    const result = await runPipeline(constant(null));
    expect(result).toBeNull();
  });

  it("returns handler output from a pipeline", async () => {
    const result = await runPipeline(
      pipe(constant({ project: "test" }), setup),
    );
    expect(result).toEqual({ initialized: true, project: "test" });
  });

  it("returns handler output from a multi-step pipeline", async () => {
    const result = await runPipeline(
      pipe(constant({ project: "test" }), setup, build),
    );
    expect(result).toEqual({ artifact: "test.build" });
  });

  it("returns the input value when using identity", async () => {
    const result = await runPipeline(identity, { data: "passthrough" });
    expect(result).toEqual({ data: "passthrough" });
  });
});
