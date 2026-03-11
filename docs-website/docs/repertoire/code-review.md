# Code review

Run a structured code review on every changed file in a PR, in parallel, for under a dollar.

## The pattern

```
                    ┌──→ CheckStandards(src/main.ts)
                    ├──→ CheckSecurity(src/main.ts)
ListChanges ────────┼──→ CheckStandards(src/utils.ts)
                    ├──→ CheckSecurity(src/utils.ts)
                    └──→ ...

After all checks complete:
finally ──→ CompileReport
```

## Why this pattern?

Some tools charge $25 per code review. Barnum runs the same checks (coding standards, security, JIRA compliance) in parallel across every changed file, each with focused context. The reviewing agent only sees its file and the relevant standards doc. No bloated context, no wasted tokens.

## Example: PR review with standards and security checks

```jsonc
{
  "entrypoint": "ListChanges",
  "steps": [
    {
      "name": "ListChanges",
      // Two review tasks per changed file
      "action": {
        "kind": "Command",
        "script": "git diff --name-only origin/main | jq -R -s 'split(\"\\n\") | map(select(length > 0)) | map([{kind: \"CheckStandards\", value: {file: .}}, {kind: \"CheckSecurity\", value: {file: .}}]) | flatten'"
      },
      "next": ["CheckStandards", "CheckSecurity"],
      // After all checks: compile a summary
      "finally": { "kind": "Command", "script": "echo '[{\"kind\": \"CompileReport\", \"value\": {}}]'" }
    },
    {
      "name": "CheckStandards",
      "value_schema": {
        "type": "object",
        "required": ["file"],
        "properties": { "file": { "type": "string" } }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "link": "instructions/check-standards.md" }
      },
      "next": []
    },
    {
      "name": "CheckSecurity",
      "value_schema": {
        "type": "object",
        "required": ["file"],
        "properties": { "file": { "type": "string" } }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "link": "instructions/check-security.md" }
      },
      "next": []
    },
    {
      "name": "CompileReport",
      "action": {
        "kind": "Pool",
        "instructions": {
          "inline": "Review all findings from the code review. Compile a summary: files reviewed, standards violations, security issues. Post a GitHub comment with the results. Return []."
        }
      },
      "next": []
    }
  ]
}
```

## Instructions files

The linked instruction files keep each reviewer focused:

**instructions/check-standards.md:**
```markdown
Read the file at the given path. Check it against the project's coding standards:

1. Read `coding-standards.md` from the repo root.
2. Compare every function, type, and import against the rules.
3. Flag any violations with line numbers and the specific rule broken.
4. Write your findings to `reviews/{file}.standards.json`.

Return `[]`.
```

**instructions/check-security.md:**
```markdown
Read the file at the given path. Review it for security issues:

1. Check for hardcoded secrets, credentials, or API keys.
2. Look for SQL injection, XSS, command injection, and path traversal.
3. Verify input validation on any user-facing functions.
4. Check for unsafe deserialization or eval usage.
5. Write your findings to `reviews/{file}.security.json`.

Return `[]`.
```

## Running

```bash
barnum run --config code-review.jsonc --pool agents --entrypoint-value '{}'
```

## How it works

1. **ListChanges** runs `git diff --name-only` and emits two tasks per changed file: one for standards, one for security.
2. **CheckStandards** and **CheckSecurity** run in parallel across all files. Each agent sees only its file and its specific instructions.
3. When every check completes, the **finally** hook fires and dispatches **CompileReport**.
4. **CompileReport** reads all the review findings and posts a summary.

## Extending this

- **Add more checks**: JIRA ticket verification, test coverage, documentation completeness. Each one is just another step and another entry in the ListChanges jq script.
- **Add a pre-hook**: Enrich each review task with the file's git blame or recent commit history.
- **Post to GitHub**: The CompileReport agent can use `gh pr comment` to post findings directly on the PR.

## Key points

- Each reviewer agent sees only one file and one concern, with minimal context and maximum focus
- All checks run in parallel. 20 files with 2 checks each = 40 tasks, all concurrent
- Linked instructions keep the config clean and let you version-control your review rules separately
- The finally hook guarantees the report only runs after every check completes
