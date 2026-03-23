import { BarnumConfig } from "@barnum/barnum";
import { createRequire } from "node:module";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const __dirname = dirname(fileURLToPath(import.meta.url));

BarnumConfig.fromConfig(require("./config.json"))
  .run({
    pool: process.env.POOL,
    root: process.env.ROOT,
    entrypointValue: JSON.stringify({ folder: __dirname }),
  })
  .on("exit", (code) => process.exit(code ?? 1));
