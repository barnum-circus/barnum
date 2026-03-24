import { BarnumConfig } from "@barnum/barnum";

await BarnumConfig.fromConfig({
  entrypoint: "Start",
  steps: [
    {
      name: "Start",
      action: {
        kind: "Bash",
        script:
          'TASK=$(cat); ${TROUPE:-pnpm dlx @barnum/troupe} submit_task --pool $BARNUM_POOL --root $BARNUM_ROOT --notify file --data "$(jq -n --arg inst \'You are at the start. Transition to Middle.\n\nReturn: `[{"kind": "Middle", "value": {}}]`\' --argjson task "$TASK" \'{task: $task, instructions: $inst}\')" | jq -r \'.stdout\'',
      },
      next: ["Middle"],
    },
    {
      name: "Middle",
      action: {
        kind: "Bash",
        script:
          'TASK=$(cat); ${TROUPE:-pnpm dlx @barnum/troupe} submit_task --pool $BARNUM_POOL --root $BARNUM_ROOT --notify file --data "$(jq -n --arg inst \'You are in the middle. Transition to End.\n\nReturn: `[{"kind": "End", "value": {}}]`\' --argjson task "$TASK" \'{task: $task, instructions: $inst}\')" | jq -r \'.stdout\'',
      },
      next: ["End"],
    },
    {
      name: "End",
      action: {
        kind: "Bash",
        script:
          'TASK=$(cat); ${TROUPE:-pnpm dlx @barnum/troupe} submit_task --pool $BARNUM_POOL --root $BARNUM_ROOT --notify file --data "$(jq -n --arg inst \'You have reached the end. Return an empty array.\n\nReturn: `[]`\' --argjson task "$TASK" \'{task: $task, instructions: $inst}\')" | jq -r \'.stdout\'',
      },
      next: [],
    },
  ],
}).run({ wake: process.env.BARNUM_WAKE });
