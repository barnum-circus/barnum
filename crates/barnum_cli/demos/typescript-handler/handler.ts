import { z } from "zod";
import { createHandler } from "@barnum/barnum";

const stepConfigValidator = z.object({});
const stepValueValidator = z.object({ name: z.string() });

export default createHandler({
  stepConfigValidator,

  getStepValueValidator(_stepConfig) {
    return stepValueValidator;
  },

  async handle({ value }) {
    return [{ kind: "Done", value: { greeting: `Hello, ${value.name}!` } }];
  },
});
