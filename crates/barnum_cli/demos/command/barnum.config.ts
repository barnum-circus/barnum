import { BarnumConfig } from "@barnum/barnum";

await BarnumConfig.fromConfig({
  entrypoint: "Split",
  steps: [
    {
      name: "Split",
      action: {
        kind: "Bash",
        script:
          "jq -c '.value.items[] | {kind: \"Process\", value: .}' | jq -s",
      },
      next: ["Process"],
    },
    {
      name: "Process",
      action: {
        kind: "Bash",
        script:
          "jq -c '{kind: \"Collect\", value: {processed: .value, doubled: (.value.n * 2)}}' | jq -s",
      },
      next: ["Collect"],
    },
    {
      name: "Collect",
      action: {
        kind: "Bash",
        script: "echo '[]'",
      },
      next: [],
    },
  ],
}).run({ entrypointValue: '{"items": [{"n": 1}, {"n": 2}, {"n": 3}]}' });
