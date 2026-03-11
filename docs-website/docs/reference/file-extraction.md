# Extracting to separate files

As configs grow, inline instructions and schemas can make the config hard to read. Barnum supports extracting both into separate files using `{"link": "path"}`.

## Linkable fields

Two fields support file linking:

| Field | Location | Description |
|-------|----------|-------------|
| `instructions` | Pool actions | Agent instructions (markdown) |
| `value_schema` | Steps | JSON Schema for task values |

## Instructions

Pool action instructions can be inline strings or linked files:

```jsonc
// Inline (short instructions)
{
  "action": {
    "kind": "Pool",
    "instructions": "Read the file. Return `[]`."
  }
}

// Linked (long instructions)
{
  "action": {
    "kind": "Pool",
    "instructions": { "link": "instructions/analyze.md" }
  }
}
```

When using `{"link": "..."}`, the path resolves relative to the config file's directory. So if your config is at `project/config.json` and the link is `instructions/analyze.md`, Barnum reads `project/instructions/analyze.md`.

The linked file contains raw instruction text, no JSON wrapping needed:

```markdown
<!-- instructions/analyze.md -->
Read the file at the given path. Determine which refactoring
approach would most improve the code.

Return one task:
- `ExtractToFile` if code should be split into a new file
- `RenameVariables` if names are unclear
- `RemoveUnusedProps` if there's dead code

Example response:
\`\`\`json
[{"kind": "ExtractToFile", "value": {"file": "src/main.rs", "target": "Config struct"}}]
\`\`\`
```

## Value schema

Schemas can be inline objects or linked files:

```jsonc
// Inline
{
  "name": "Analyze",
  "value_schema": {
    "type": "object",
    "required": ["file"],
    "properties": {
      "file": { "type": "string" }
    }
  }
}

// Linked
{
  "name": "Analyze",
  "value_schema": { "link": "schemas/analyze.json" }
}
```

The linked file contains a plain JSON Schema:

```json
{
  "type": "object",
  "required": ["file"],
  "properties": {
    "file": { "type": "string" }
  }
}
```

## Example: extracted config

A config directory might look like:

```
project/
├── config.json
├── instructions/
│   ├── analyze.md
│   ├── extract-to-file.md
│   └── rename-variables.md
└── schemas/
    ├── analyze.json
    └── refactor-target.json
```

And the config stays clean:

```jsonc
{
  "entrypoint": "Analyze",
  "steps": [
    {
      "name": "Analyze",
      "value_schema": { "link": "schemas/analyze.json" },
      "action": {
        "kind": "Pool",
        "instructions": { "link": "instructions/analyze.md" }
      },
      "next": ["ExtractToFile", "RenameVariables"]
    },
    {
      "name": "ExtractToFile",
      "value_schema": { "link": "schemas/refactor-target.json" },
      "action": {
        "kind": "Pool",
        "instructions": { "link": "instructions/extract-to-file.md" }
      },
      "next": []
    },
    {
      "name": "RenameVariables",
      "value_schema": { "link": "schemas/refactor-target.json" },
      "action": {
        "kind": "Pool",
        "instructions": { "link": "instructions/rename-variables.md" }
      },
      "next": []
    }
  ]
}
```

## When to extract

- **Extract instructions** when they're more than a few sentences. Markdown files get syntax highlighting in your editor and are easier to review in PRs.
- **Extract schemas** when they're reused across steps or are complex enough to benefit from their own file.
- **Keep inline** for one-liners like `"Read the file. Return \`[]\`."`. Linking would add ceremony without improving readability.
