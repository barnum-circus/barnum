import { barnumRun } from "@barnum/barnum";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));

barnumRun({
  config: join(__dirname, "config.jsonc"),
  pool: process.env.POOL,
  root: process.env.ROOT,
  entrypointValue: JSON.stringify({ folder: __dirname }),
}).on("exit", (code) => process.exit(code ?? 1));
