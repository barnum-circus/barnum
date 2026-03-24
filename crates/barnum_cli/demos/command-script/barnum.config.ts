import { BarnumConfig } from "@barnum/barnum";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));

BarnumConfig.fromConfig({
  entrypoint: "ListFiles",
  options: {
    maxRetries: 1,
  },
  steps: [
    {
      name: "ListFiles",
      action: {
        kind: "Bash",
        script: "./list-files.sh",
      },
      next: ["AnalyzeFile"],
    },
    {
      name: "AnalyzeFile",
      action: {
        kind: "Bash",
        script: "echo '[]'",
      },
      next: [],
    },
  ],
})
  .run({ entrypointValue: JSON.stringify({ folder: __dirname }), cwd: __dirname })
  .on("exit", (code) => process.exit(code ?? 1));
