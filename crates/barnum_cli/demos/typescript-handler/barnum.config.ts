import { BarnumConfig } from "@barnum/barnum";
import { resolve } from "node:path";

BarnumConfig.fromConfig({
  entrypoint: "Greet",
  steps: [
    {
      name: "Greet",
      action: {
        kind: "TypeScript",
        path: resolve(import.meta.dirname, "handler.ts"),
      },
      next: ["Done"],
    },
    {
      name: "Done",
      action: {
        kind: "Bash",
        script: "cat > /dev/null; echo '[]'",
      },
      next: [],
    },
  ],
})
  .run({
    entrypointValue: JSON.stringify({ name: "World" }),
    wake: process.env.BARNUM_WAKE,
  })
  .on("exit", (code) => process.exit(code ?? 1));
