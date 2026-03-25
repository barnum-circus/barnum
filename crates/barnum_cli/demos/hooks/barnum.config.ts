import { BarnumConfig } from "@barnum/barnum";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));

const child = await BarnumConfig.fromConfig({
  entrypoint: "Process",
  options: {
    maxRetries: 0,
  },
  steps: [
    {
      name: "Process",
      action: {
        kind: "Bash",
        script: "./process.sh",
      },
      finally: { kind: "Bash", script: "./finally-hook.sh" },
      next: ["Cleanup"],
    },
    {
      name: "Cleanup",
      action: {
        kind: "Bash",
        script: "echo '[]'",
      },
      next: [],
    },
  ],
}).run({ entrypointValue: { item: "test-item" }, cwd: __dirname });
child.on("exit", (code) => process.exit(code ?? 1));
