import type { z } from "zod";

export interface HandlerDefinition<C = unknown, V = unknown> {
  stepConfigValidator: z.ZodType<C>;
  getStepValueValidator: (stepConfig: C) => z.ZodType<V>;
  handle: (context: HandlerContext<C, V>) => Promise<FollowUpTask[]>;
}

export interface HandlerContext<C, V> {
  stepConfig: C;
  value: V;
  config: unknown;
  stepName: string;
}

export interface FollowUpTask {
  kind: string;
  value: unknown;
}
