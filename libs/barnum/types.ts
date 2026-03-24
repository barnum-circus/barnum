import type { z } from "zod";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export interface HandlerDefinition<C = unknown, V = unknown> {
  stepConfigValidator: z.ZodType<C, z.ZodTypeDef, any>;
  getStepValueValidator: (stepConfig: C) => z.ZodType<V, z.ZodTypeDef, any>;
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
