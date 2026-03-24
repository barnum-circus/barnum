import { BarnumConfig } from "@barnum/barnum";

const child = await BarnumConfig.fromConfig({
  entrypoint: "Start",
  steps: [
    {
      name: "Start",
      action: {
        kind: "Bash",
        script:
          "TASK=$(cat); ${TROUPE:-pnpm dlx @barnum/troupe} submit_task --pool $BARNUM_POOL --root $BARNUM_ROOT --notify file --data \"$(jq -n --arg inst 'This is the starting step. Return an empty array to finish.' --argjson task \"$TASK\" '{task: $task, instructions: $inst}')\" | jq -r '.stdout'",
      },
      next: [],
    },
  ],
}).run({ wake: process.env.BARNUM_WAKE });
child.on("exit", (code) => process.exit(code ?? 1));
