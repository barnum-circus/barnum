/*
Run from repo root:
BARNUM=./target/debug/barnum pnpm dlx tsx crates/barnum_cli/demos/simple/run-demo.ts
*/
import { BarnumConfig } from "@barnum/barnum";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);

BarnumConfig.fromConfig(require("./config.json"))
  .run()
  .on("exit", (code) => process.exit(code ?? 1));
