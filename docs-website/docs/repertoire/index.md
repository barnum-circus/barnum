# Barnum Repertoire

Each routine below is a self-contained workflow you can copy, adapt, and combine.

## Quick Reference

| Routine | Description |
|--------|-------------|
| [Linear Pipeline](linear-pipeline.md) | Step-by-step processing (A → B → C) |
| [Branching](branching.md) | Conditional paths based on output |
| [Fan-Out](fan-out.md) | Split one task into many parallel tasks |
| [Fan-Out with Finally](fan-out-finally.md) | Parallel changes with commit on completion |
| [Sequential Processing](sequential.md) | Enforce single-threaded task execution |
| [Adversarial Review](adversarial-review.md) | Implement → judge → revise loop |
| [Branching Refactor](branching-refactor.md) | Route to specialized agents based on analysis |
| [Error Recovery](error-recovery.md) | Catch failures and route to recovery steps |
| [Pre/Post/Finally Hooks](hooks.md) | Transform data, aggregate results, cleanup |
| [Validation](validation.md) | Schema validation for inputs and outputs |
| [Local Commands](commands.md) | Run shell scripts instead of agents |
| [Code Review](code-review.md) | Parallel PR review with standards and security checks |
| [Legal Review](legal-review.md) | Parallel contract analysis with final recommendation |

## Terminology

- **Step**: A named stage in your workflow (e.g., "Analyze", "Implement", "Review")
- **Task**: An instance of a step with a specific value (e.g., `{step: "Analyze", value: {file: "main.rs"}}`)
- **Action**: What happens when a task runs (Pool = send to agent, Command = run locally)
- **Transition**: Moving from one step to another via the `next` field
- **Hook**: A shell command that runs before (pre), after (post), or when descendants complete (finally)
