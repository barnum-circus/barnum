/*
Run from repo root:
BARNUM=./target/debug/barnum pnpm dlx tsx crates/barnum_cli/demos/command-script/run-demo.ts
*/
import { BarnumConfig } from "@barnum/barnum";
import { createRequire } from "node:module";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const __dirname = dirname(fileURLToPath(import.meta.url));

BarnumConfig.fromConfig(require("./config.json"))
  .run({
    entrypointValue: JSON.stringify({ folder: __dirname }),
  })
  .on("exit", (code) => process.exit(code ?? 1));
