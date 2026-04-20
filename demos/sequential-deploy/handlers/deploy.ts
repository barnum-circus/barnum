import { createHandler } from "@barnum/barnum/runtime";
import { z } from "zod";

/** Return the services in dependency order. First must deploy before second, etc. */
export const getServices = createHandler(
  {
    outputValidator: z.array(z.string()),
    handle: async () => {
      console.error("[getServices] Resolving dependency order...");
      return ["database", "cache", "auth", "api", "frontend"];
    },
  },
  "getServices",
);

/** Deploy a single service. Takes the service name, returns the service name. */
export const deployService = createHandler(
  {
    inputValidator: z.string(),
    outputValidator: z.string(),
    handle: async ({ value: service }) => {
      const delay = 500 + Math.floor(Math.random() * 1000);
      console.error(`[deploy] Deploying ${service}...`);
      await new Promise((resolve) => setTimeout(resolve, delay));
      console.error(`[deploy] ${service} deployed (${delay}ms)`);
      return service;
    },
  },
  "deployService",
);

/** Verify a service is healthy after deployment. Takes the service name, returns the service name. */
export const verifyService = createHandler(
  {
    inputValidator: z.string(),
    outputValidator: z.string(),
    handle: async ({ value: service }) => {
      console.error(`[verify] Health-checking ${service}...`);
      await new Promise((resolve) => setTimeout(resolve, 200));
      console.error(`[verify] ${service} healthy`);
      return service;
    },
  },
  "verifyService",
);
