import { existsSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";

import { CompiledWorkflow, config, constant } from "../src/index.js";

const HERE = import.meta.dirname;
const BINARY = path.resolve(HERE, "../../../target/debug/barnum");
const HAS_BINARY = existsSync(BINARY);

describe("compile()", () => {
  it("produces a CompiledWorkflow whose configJson matches the serialized config", () => {
    const pipeline = constant({ value: 42 });
    const compiled = pipeline.compile();
    expect(compiled).toBeInstanceOf(CompiledWorkflow);
    expect(compiled.configJson).toEqual(JSON.stringify(config(pipeline)));
  });

  it("compile() can be re-run, yielding equal config JSON each time", () => {
    const pipeline = constant({ value: 42 });
    expect(pipeline.compile().configJson).toEqual(
      pipeline.compile().configJson,
    );
  });

  it("CompiledWorkflow.fromJSON wraps a raw config-JSON string", () => {
    const json = JSON.stringify(config(constant({ value: 42 })));
    const compiled = CompiledWorkflow.fromJSON(json);
    expect(compiled).toBeInstanceOf(CompiledWorkflow);
    expect(compiled.configJson).toEqual(json);
  });
});

describe.skipIf(!HAS_BINARY)("run()", () => {
  it("pipeline.compile().run() returns the final value", async () => {
    const result = await constant({ value: 42 }).compile().run();
    expect(result).toEqual({ value: 42 });
  });

  it("pipeline.run() is sugar for compile().run()", async () => {
    const result = await constant({ value: 7 }).run();
    expect(result).toEqual({ value: 7 });
  });

  it("input is supplied by narrowing In to null via call()", async () => {
    const addOne = constant(10);
    const result = await addOne.call(constant(null)).run();
    expect(result).toEqual(10);
  });

  it("CompiledWorkflow.fromJSON(json).run() executes the wrapped config", async () => {
    const json = JSON.stringify(config(constant({ value: 99 })));
    const result = await CompiledWorkflow.fromJSON(json).run();
    expect(result).toEqual({ value: 99 });
  });
});
