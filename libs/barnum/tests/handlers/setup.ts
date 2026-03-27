import { createHandler } from "../../src/core.js";

export type SetupInput = { project: string };
export type SetupOutput = { initialized: boolean; project: string };

export default createHandler<SetupInput, SetupOutput>({
  handle: async (input) => ({ initialized: true, project: input.project }),
});
