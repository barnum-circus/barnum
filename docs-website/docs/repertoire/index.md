---
image: /img/og/repertoire.png
---

# Repertoire

Each pattern below is a self-contained workflow you can copy, adapt, and combine.

| Pattern | Combinator | Description |
|---|---|---|
| [Linear Pipeline](./linear-pipeline.md) | `pipe` | Sequential steps: A → B → C |
| [Fan-Out](./fan-out.md) | `forEach` | Split one task into parallel tasks |
| [Fan-Out with Aggregation](./fan-out-finally.md) | `forEach` + `pipe` | Parallel work, then a follow-up step |
| [Sequential Processing](./sequential.md) | handler loop | Process items one at a time, in order |
| [Branching](./branching.md) | `branch` | Conditional paths based on output |
| [Branching Refactor](./branching-refactor.md) | `branch` | Route to specialized agents |
| [Adversarial Review](./adversarial-review.md) | `loop` + `branch` | Implement → judge → revise loop |
| [Error Recovery](./error-recovery.md) | `tryCatch` | Catch failures, route to recovery |
| [Side Effects](./hooks.md) | `tap` | Run actions for side effects |
| [Validation](./validation.md) | Zod | Schema validation for inputs and outputs |
| [Deterministic Steps](./commands.md) | handlers | Use TypeScript for non-agent work |
| [Code Review](./code-review.md) | `forEach` + `all` | Parallel multi-check review |
| [Legal Review](./legal-review.md) | `all` + `pipe` | Parallel analysis + synthesis |
| [Document Verification](./document-verification.md) | `forEach` | Extract facts, verify each |
| [Editing Assistant](./editing-assistant.md) | `all` | Parallel writing checks |
