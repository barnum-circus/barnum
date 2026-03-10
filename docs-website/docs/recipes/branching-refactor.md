# Branching Refactor

Use branching to route a file to a specialized refactoring agent based on what it needs.

## Why This Pattern?

The key insight: **each refactoring agent only needs instructions for its specific refactor type.** The Analyze step figures out *what* to do, then dispatches to a step whose instructions are laser-focused on *how* to do that one thing. This keeps agent context small and focused.

## The Pattern

```
              ┌─→ ExtractToFile ──→ Done
              │
Analyze ──────┼─→ RenameVariables ──→ Done
              │
              └─→ RemoveUnusedProps ──→ Done
```

## Example: Targeted File Refactoring

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
        "instructions": { "inline": "Read the file at the given path. Determine which ONE of the following refactors would most improve the code:\n\n1. **ExtractToFile** — A section of the file (a class, a group of related functions, a set of constants) should be extracted into its own file.\n2. **RenameVariables** — Variables, functions, or types have unclear names that hurt readability.\n3. **RemoveUnusedProps** — There are unused imports, function parameters, struct fields, or type properties that should be removed.\n\nReturn exactly one task for the appropriate refactor type. Include the file path and a description of what specifically should be refactored:\n\n```json\n[{\"kind\": \"ExtractToFile\", \"value\": {\"file\": \"src/main.rs\", \"target\": \"The Config struct and its impl block (lines 15-80) should be extracted to src/config.rs\"}}]\n```\n\nOr:\n```json\n[{\"kind\": \"RenameVariables\", \"value\": {\"file\": \"src/main.rs\", \"target\": \"Variables x, tmp, and val on lines 30-45 should be renamed to reflect their purpose\"}}]\n```\n\nOr:\n```json\n[{\"kind\": \"RemoveUnusedProps\", \"value\": {\"file\": \"src/main.rs\", \"target\": \"The 'debug_mode' field on Config and the 'legacy_format' parameter on parse() are never used\"}}]\n```" }
      },
      "next": ["ExtractToFile", "RenameVariables", "RemoveUnusedProps"]
    },
    {
      "name": "ExtractToFile",
      "value_schema": {
        "type": "object",
        "required": ["file", "target"],
        "properties": {
          "file": { "type": "string" },
          "target": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "inline": "Extract the specified code into a new file.\n\n1. Read the source file.\n2. Identify the code described in `target`.\n3. Create a new file with the extracted code.\n4. Add appropriate imports/exports in both files.\n5. Update the original file to import from the new file.\n6. Write both files to disk.\n\nReturn `[]` when done." }
      },
      "next": []
    },
    {
      "name": "RenameVariables",
      "value_schema": {
        "type": "object",
        "required": ["file", "target"],
        "properties": {
          "file": { "type": "string" },
          "target": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "inline": "Rename variables for clarity.\n\n1. Read the file.\n2. Identify the variables/functions/types described in `target`.\n3. Choose clear, descriptive names that convey intent.\n4. Rename all occurrences consistently throughout the file.\n5. Write the file to disk.\n\nReturn `[]` when done." }
      },
      "next": []
    },
    {
      "name": "RemoveUnusedProps",
      "value_schema": {
        "type": "object",
        "required": ["file", "target"],
        "properties": {
          "file": { "type": "string" },
          "target": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "inline": "Remove unused code.\n\n1. Read the file.\n2. Identify the unused items described in `target`.\n3. Remove them along with any now-unnecessary imports.\n4. Verify the remaining code still compiles/makes sense.\n5. Write the file to disk.\n\nReturn `[]` when done." }
      },
      "next": []
    }
  ]
}
```

## Running

```bash
barnum run --config config.json --pool agents --entrypoint-value '{"file": "src/main.rs"}'
```

## How It Works

1. **Analyze** reads the file and decides which refactor type is most needed.
2. The agent returns exactly one task — routing to the appropriate specialized step.
3. The specialized step (e.g., **RenameVariables**) receives only the file path and a description of what to rename. Its instructions are focused entirely on how to do *that specific refactor*. It doesn't know about the other refactor types.

## Why Not One Big Step?

You could put all three refactors in a single step's instructions, but splitting them has advantages:

- **Focused context**: Each refactoring agent sees only the instructions relevant to its task. An agent doing variable renames doesn't need to know how to extract code into files.
- **Independent retry policies**: You can set a longer timeout for `ExtractToFile` (which involves creating new files) vs. `RenameVariables` (which is simpler).
- **Composability**: The specialized steps can be reused in other workflows.

## Key Points

- The `next` array on Analyze lists all three possible refactor types
- The Analyze agent picks exactly one based on what the file needs
- Each refactor step has narrow, focused instructions
- Invalid transitions (returning a `kind` not in `next`) trigger retries
