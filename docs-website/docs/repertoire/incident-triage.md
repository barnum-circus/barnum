# Incident Triage

When an alert fires, automatically collect context from multiple systems in parallel, analyze it, and produce a triage document — before a human even opens the page.

## Workflow

```ts
runPipeline(
  pipe(
    all(
      collectLogs,
      collectMetrics,
      collectRecentDeploys,
      queryBusinessIntelligence,
    ),
    merge(),
    all(
      correlateEvents,
      classifySeverity,
      identifyAffectedServices,
    ),
    merge(),
    draftTriageDocument,
  ),
);
```

## Stages

1. **Parallel data collection** — gather everything relevant concurrently:
   - **Logs** — query your logging system for error spikes around the alert time.
   - **Metrics** — pull latency, error rate, and throughput from your monitoring system.
   - **Recent deploys** — ask an LLM (or query a deploy tracker) for what shipped in the last 24 hours. If the deploy service has an API, this is deterministic. If not, an agent can query it via CLI or chat.
   - **Business intelligence** — pull relevant data from internal BI tools (revenue impact, affected user count, feature flag states). This is the kind of data that's crucial for triage but often forgotten because it requires logging into a separate system.
2. **Merge** — combine the four data sources into a single context object.
3. **Parallel analysis** — three focused agents run concurrently:
   - **Correlate events** — find causal links between deploys, metric changes, and log patterns.
   - **Classify severity** — determine if this is a P0, P1, or P2 based on impact data.
   - **Identify affected services** — trace the blast radius from the error source.
4. **Draft triage document** — synthesize all analyses into an actionable document for the on-call engineer: what happened, what's affected, probable cause, and recommended next steps.

## Example: deploy correlation handler

```ts
export const collectRecentDeploys = createHandler({
  inputValidator: z.object({ alertTime: z.string(), service: z.string() }),
  outputValidator: z.object({
    deploys: z.array(z.object({
      service: z.string(),
      time: z.string(),
      author: z.string(),
      description: z.string(),
    })),
  }),
  handle: async ({ value }) => {
    const response = await callClaude({
      prompt: `Find all deploys to ${value.service} and related services in the 24 hours before ${value.alertTime}. Use the deploy tracker CLI to query recent deployments.`,
      allowedTools: ["Bash"],
    });
    return JSON.parse(response);
  },
}, "collectRecentDeploys");
```

## Example: business intelligence handler

```ts
export const queryBusinessIntelligence = createHandler({
  inputValidator: z.object({ alertTime: z.string(), service: z.string() }),
  outputValidator: z.object({
    revenueImpact: z.string(),
    affectedUsers: z.number(),
    featureFlags: z.array(z.object({ name: z.string(), enabled: z.boolean() })),
  }),
  handle: async ({ value }) => {
    const response = await callClaude({
      prompt: `Query the BI dashboard for impact data related to ${value.service} around ${value.alertTime}. Get: approximate revenue impact, number of affected users, and any recently changed feature flags.`,
      allowedTools: ["Bash", "Read"],
    });
    return JSON.parse(response);
  },
}, "queryBusinessIntelligence");
```

## Key points

- Two levels of `all`: data collection (I/O-bound, different systems) and analysis (focused LLM calls). Both benefit from parallelism.
- `merge()` combines tuple outputs into a single object, so downstream steps don't need to destructure tuples.
- The triage document drafter sees the full picture — correlations, severity, affected services, business impact — but never queries any system directly.
- Each data collector is independent and can fail independently. Wrap individual collectors in `tryCatch` to degrade gracefully if a system is unreachable.
- Add `withTimeout` around the entire pipeline to ensure triage completes within SLA.
