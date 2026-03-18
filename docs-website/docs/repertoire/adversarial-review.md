---
image: /img/og/repertoire-adversarial-review.png
---

# Adversarial Review

Use a feedback loop where one agent implements changes and another judges them, iterating until the result passes review.

## The Pattern

```
              ┌────────────────────────────┐
              │                            │
              ▼                            │
Analyze ──→ Refactor ──→ Judge ──┬──→ Done │
                                 │         │
                                 └─────────┘
                              (with feedback)
```

## Example: Refactor with Review Loop

Analyze a file, implement a refactor, then have a separate agent judge the result. The judge either approves (done) or sends it back with feedback.

```jsonc
{
  "entrypoint": "Analyze",
  "steps": [
    {
      "name": "Analyze",
      "value_schema": {
        "type": "object",
        "required": ["file"],
        "properties": {
          "file": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "inline": "Read the file at the given path. Identify ONE concrete refactoring opportunity (e.g., extract a function, simplify a conditional, remove duplication).\n\nReturn a task for the Refactor step with the file path and a description of the refactoring to perform:\n```json\n[{\"kind\": \"Refactor\", \"value\": {\"file\": \"src/main.rs\", \"instructions\": \"Extract the validation logic on lines 45-60 into a separate validate_input() function\", \"feedback\": null}}]\n```" }
      },
      "next": ["Refactor"]
    },
    {
      "name": "Refactor",
      "value_schema": {
        "type": "object",
        "required": ["file", "instructions"],
        "properties": {
          "file": { "type": "string" },
          "instructions": { "type": "string" },
          "feedback": { "type": ["string", "null"] }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "inline": "Read the file and apply the refactoring described in `instructions`. Write the changes back to disk.\n\nIf `feedback` is present, a previous attempt was rejected by the reviewer. Address the feedback in your implementation.\n\nReturn a task for the Judge step with the file path and what you changed:\n```json\n[{\"kind\": \"Judge\", \"value\": {\"file\": \"src/main.rs\", \"instructions\": \"Extract validate_input()\", \"description\": \"Extracted lines 45-60 into validate_input() and replaced with a call\"}}]\n```" }
      },
      "next": ["Judge"]
    },
    {
      "name": "Judge",
      "value_schema": {
        "type": "object",
        "required": ["file", "instructions", "description"],
        "properties": {
          "file": { "type": "string" },
          "instructions": { "type": "string" },
          "description": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "inline": "You are a code reviewer. Read the file and evaluate whether the refactoring was done correctly.\n\nThe original instructions were in `instructions`. The implementer described what they did in `description`. Read the actual file to verify.\n\nIf the refactoring is correct, clean, and complete, approve it:\n```json\n[]\n```\n\nIf there are problems, send it back to Refactor with specific feedback:\n```json\n[{\"kind\": \"Refactor\", \"value\": {\"file\": \"src/main.rs\", \"instructions\": \"Extract validate_input()\", \"feedback\": \"The extracted function still references a local variable 'config' from the original scope. Pass it as a parameter instead.\"}}]\n```\n\nBe rigorous. Only approve if the code is genuinely better than before." }
      },
      "next": ["Refactor"]
    }
  ]
}
```

## Running

```bash
barnum run --config config.json --entrypoint-value '{"file": "src/main.rs"}'
```

## How It Works

1. **Analyze** reads the file and identifies a refactoring opportunity. Returns a `Refactor` task with instructions.
2. **Refactor** applies the change. If this is a retry, `feedback` contains the judge's critique from the previous round. Returns a `Judge` task.
3. **Judge** reviews the result. If acceptable, returns `[]` (done). If not, returns a `Refactor` task with `feedback` explaining what to fix.
4. Steps 2-3 repeat until the judge approves.

## Tips

- Set `max_retries` to limit the review loop (e.g., `"max_retries": 5` on the Judge step prevents infinite back-and-forth)
- The `feedback` field is `null` on the first attempt and a string on subsequent attempts, so the Refactor agent knows whether it's a first try or a revision
- Both the Refactor and Judge agents have access to the original `instructions`, preserving context across iterations

## Key Points

- Self-referencing `next` enables feedback loops (Judge → Refactor → Judge)
- The `feedback` field carries reviewer critique into the next iteration
- Different agents handle implementation vs. review, preventing self-approval bias
- The loop terminates when the judge returns `[]`
