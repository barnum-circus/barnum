import { BarnumConfig } from "@barnum/barnum";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);

BarnumConfig.fromConfig(require("./config.json"))
  .run({
    pool: process.env.POOL,
    root: process.env.ROOT,
    entrypointValue: '{"items": [{"n": 1}, {"n": 2}, {"n": 3}]}',
  })
  .on("exit", (code) => process.exit(code ?? 1));
