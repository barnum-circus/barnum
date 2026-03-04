# Documentation

**Status:** Not started

## Motivation

GSD needs documentation for users to adopt it. Current state is code-only with no guides, examples, or API docs.

## Required Documentation

### 1. README.md (root)

Quick start guide:

```markdown
# GSD - Get Sh*** Done

JSON-based task orchestrator for AI agent workflows.

## Installation

\`\`\`bash
npm install -g @gsd-now/gsd @gsd-now/agent-pool
\`\`\`

## Quick Start

1. Create a config file (config.jsonc)
2. Start a pool: \`agent_pool start --pool mypool\`
3. Run: \`gsd run config.jsonc --pool mypool\`

## Features

- Directed acyclic graph of tasks
- Multiple agent pools
- File-based and socket-based communication
- Resume interrupted runs
\`\`\`

### 2. docs/CONFIG.md

Config file format documentation:

- Top-level fields (options, steps, initial)
- Step types (Pool, Command, Sequence)
- Output routing and transforms
- Value schemas for type safety
- Examples for common patterns

### 3. docs/PROTOCOL.md

Agent protocol documentation:

- Anonymous worker protocol flow
- File formats (ready.json, task.json, response.json)
- Heartbeat mechanism
- Error handling

### 4. docs/EXAMPLES.md

Worked examples:

- Simple single-step workflow
- Multi-step with dependencies
- Parallel processing with fan-out/fan-in
- Using command steps for local execution

### 5. Agent Instructions

Update `AGENT_INSTRUCTIONS.md` in protocols/ to be more user-friendly and complete.

## Implementation Approach

1. Start with README - essential for npm page
2. Add config docs - most commonly needed
3. Add protocol docs - for agent developers
4. Add examples - for learning

## Open Questions

1. Where should docs live? `docs/` folder or separate site?
2. Generate API docs from code comments?
3. Include architecture diagrams?

## Files to Create

- `README.md` - root readme (currently empty or basic)
- `docs/CONFIG.md` - config format docs
- `docs/PROTOCOL.md` - agent protocol docs
- `docs/EXAMPLES.md` - worked examples
