import { BarnumConfig } from "@barnum/barnum";

const child = await BarnumConfig.fromConfig({
  entrypoint: "ListFiles",
  options: {
    maxRetries: 2,
  },
  steps: [
    {
      name: "ListFiles",
      action: {
        kind: "Bash",
        script:
          "TASK=$(cat); INST=$(cat list-files.md); ${TROUPE:-pnpm dlx @barnum/troupe} submit_task --pool $BARNUM_POOL --root $BARNUM_ROOT --notify file --data \"$(jq -n --arg inst \"$INST\" --argjson task \"$TASK\" '{task: $task, instructions: $inst}')\" | jq -r '.stdout'",
      },
      next: ["AnalyzeFile"],
    },
    {
      name: "AnalyzeFile",
      action: {
        kind: "Bash",
        script:
          "TASK=$(cat); INST=$(cat analyze-file.md); ${TROUPE:-pnpm dlx @barnum/troupe} submit_task --pool $BARNUM_POOL --root $BARNUM_ROOT --notify file --data \"$(jq -n --arg inst \"$INST\" --argjson task \"$TASK\" '{task: $task, instructions: $inst}')\" | jq -r '.stdout'",
      },
      next: ["ProcessRefactorList"],
    },
    {
      name: "ProcessRefactorList",
      action: {
        kind: "Bash",
        script:
          "TASK=$(cat); INST=$(cat process-refactor-list.md); ${TROUPE:-pnpm dlx @barnum/troupe} submit_task --pool $BARNUM_POOL --root $BARNUM_ROOT --notify file --data \"$(jq -n --arg inst \"$INST\" --argjson task \"$TASK\" '{task: $task, instructions: $inst}')\" | jq -r '.stdout'",
      },
      next: ["ProcessRefactorList", "CommitFile"],
    },
    {
      name: "CommitFile",
      action: {
        kind: "Bash",
        script:
          "TASK=$(cat); INST=$(cat commit-file.md); ${TROUPE:-pnpm dlx @barnum/troupe} submit_task --pool $BARNUM_POOL --root $BARNUM_ROOT --notify file --data \"$(jq -n --arg inst \"$INST\" --argjson task \"$TASK\" '{task: $task, instructions: $inst}')\" | jq -r '.stdout'",
      },
      next: [],
    },
  ],
}).run({ wake: process.env.BARNUM_WAKE });
child.on("exit", (code) => process.exit(code ?? 1));
