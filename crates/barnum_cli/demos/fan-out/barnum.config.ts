import { BarnumConfig } from "@barnum/barnum";

BarnumConfig.fromConfig({
  entrypoint: "Distribute",
  steps: [
    {
      name: "Distribute",
      action: {
        kind: "Bash",
        script:
          'TASK=$(cat); ${TROUPE:-pnpm dlx @barnum/troupe} submit_task --pool $BARNUM_POOL --root $BARNUM_ROOT --notify file --data "$(jq -n --arg inst \'Fan out to 10 parallel workers. Return an array with 10 Worker tasks.\n\nReturn:\n```json\n[\n  {"kind": "Worker", "value": {"id": 1}},\n  {"kind": "Worker", "value": {"id": 2}},\n  ...\n  {"kind": "Worker", "value": {"id": 10}}\n]\n```\' --argjson task "$TASK" \'{task: $task, instructions: $inst}\')" | jq -r \'.stdout\'',
      },
      next: ["Worker"],
    },
    {
      name: "Worker",
      action: {
        kind: "Bash",
        script:
          'TASK=$(cat); ${TROUPE:-pnpm dlx @barnum/troupe} submit_task --pool $BARNUM_POOL --root $BARNUM_ROOT --notify file --data "$(jq -n --arg inst \'Process your assigned work item.\n\nInput: `{"id": <number>}`\n\nReturn `[]` when done.\' --argjson task "$TASK" \'{task: $task, instructions: $inst}\')" | jq -r \'.stdout\'',
      },
      next: [],
    },
  ],
})
  .run({ wake: process.env.BARNUM_WAKE })
  .on("exit", (code) => process.exit(code ?? 1));
