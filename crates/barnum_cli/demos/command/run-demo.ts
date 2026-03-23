/*
Run from repo root:
BARNUM=./target/debug/barnum pnpm dlx tsx crates/barnum_cli/demos/command/run-demo.ts
*/
import { BarnumConfig } from "@barnum/barnum";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);

BarnumConfig.fromConfig(require("./config.json"))
  .run({
    entrypointValue: '{"items": [{"n": 1}, {"n": 2}, {"n": 3}]}',
  })
  .on("exit", (code) => process.exit(code ?? 1));
