/*
Run from repo root:
BARNUM=./target/debug/barnum ROOT=/tmp/troupe POOL=demo pnpm dlx tsx crates/barnum_cli/demos/hooks/run-demo.ts
*/
import { BarnumConfig } from "@barnum/barnum";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);

BarnumConfig.fromConfig(require("./config.json"))
  .run({
    pool: process.env.POOL,
    root: process.env.ROOT,
    entrypointValue: '{"item": "test-item"}',
  })
  .on("exit", (code) => process.exit(code ?? 1));
