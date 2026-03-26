import { BarnumConfig } from "@barnum/barnum";
import handler from "./handler.js";

const child = BarnumConfig.fromConfig({
  entrypoint: "Greet",
  steps: [
    {
      name: "Greet",
      action: handler,
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
}).run({
  entrypointValue: { name: "World" },
  wake: process.env.BARNUM_WAKE,
});
child.on("exit", (code) => process.exit(code ?? 1));
