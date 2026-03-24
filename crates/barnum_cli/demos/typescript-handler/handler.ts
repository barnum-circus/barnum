import { z } from "zod";
import type { HandlerDefinition } from "@barnum/barnum";

const stepConfigValidator = z.object({});
type StepConfig = z.infer<typeof stepConfigValidator>;

const stepValueValidator = z.object({ name: z.string() });
type StepValue = z.infer<typeof stepValueValidator>;

export default {
  stepConfigValidator,

  getStepValueValidator(_stepConfig) {
    return stepValueValidator;
  },

  async handle({ value }) {
    return [{ kind: "Done", value: { greeting: `Hello, ${value.name}!` } }];
  },
} satisfies HandlerDefinition<StepConfig, StepValue>;
