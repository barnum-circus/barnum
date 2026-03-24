import { BarnumConfig } from "@barnum/barnum";

BarnumConfig.fromConfig({
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
})
  .run({ entrypointValue: '{"item": "test-item"}' })
  .on("exit", (code) => process.exit(code ?? 1));
