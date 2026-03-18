---
image: /img/og/repertoire-editing-assistant.png
---

# Editing Assistant

Fan out a writing analysis into parallel checks — thesis clarity, logical rigor, and structural flow — each writing results to a separate file.

## Why This Pattern?

Good editing requires looking at a piece through multiple lenses simultaneously. Rather than one agent trying to do everything, fan out to specialists that each focus on a single dimension of quality, writing their findings to files in a shared output directory.

## The Pattern

```
FanOut ──┬──→ Thesis
         ├──→ UnsupportedClaims
         └──→ StructuralFlow
```

All three analyses run in parallel and write to separate files. No synthesis step — the output files are the deliverable.

## Example: Essay review

```jsonc
{
  "entrypoint": "FanOut",
  "options": {
    "max_concurrency": 3
  },
  "steps": [
    {
      "name": "FanOut",
      "value_schema": {
        "type": "object",
        "required": ["input_file", "output_dir"],
        "properties": {
          "input_file": { "type": "string" },
          "output_dir": { "type": "string" }
        }
      },
      "action": {
        "kind": "Command",
        // Create output directory and dispatch all three analyses.
        "script": "INPUT=$(cat) && OUT=$(echo \"$INPUT\" | jq -r '.value.output_dir') && mkdir -p \"$OUT\" && echo \"$INPUT\" | jq -c '[{kind: \"Thesis\", value: .value}, {kind: \"UnsupportedClaims\", value: .value}, {kind: \"StructuralFlow\", value: .value}]'"
      },
      "next": ["Thesis", "UnsupportedClaims", "StructuralFlow"]
    },
    {
      "name": "Thesis",
      "value_schema": {
        "type": "object",
        "required": ["input_file", "output_dir"],
        "properties": {
          "input_file": { "type": "string" },
          "output_dir": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        // TIP: Add your own style guide rules to these instructions.
        // What counts as a thesis? How explicit must it be?
        // Is an implied thesis acceptable in your genre?
        "instructions": { "inline": "Identify the thesis from the first ~500 words of the piece.\n\n1. Read the first ~500 words of the file at `input_file`.\n2. Identify the **thesis** — the central claim or argument. It may be:\n   - **Explicit**: a single sentence stating the argument\n   - **Implicit**: the argument emerges but is never directly stated\n   - **Absent**: the opening is descriptive or meandering without committing to a position\n3. Evaluate:\n   - **Clarity**: Can you state the thesis in one sentence?\n   - **Specificity**: Is it falsifiable/debatable, or a vague truism?\n   - **Placement**: Does the reader know the thesis within 500 words?\n4. Write your analysis to `{output_dir}/thesis.md` using write_file. Use this format:\n\n```\n# Thesis Analysis\n\n## Thesis\n[One sentence, or note that none was found]\n\n## Assessment\n- **Clarity**: clear / muddy / absent\n- **Specificity**: specific and debatable / vague truism / too broad\n- **Placement**: stated by word N / implied but never stated / absent\n\n## Recommendations\n[Concrete suggestions]\n```\n\nReturn `[]` when done." }
      },
      "next": []
    },
    {
      "name": "UnsupportedClaims",
      "value_schema": {
        "type": "object",
        "required": ["input_file", "output_dir"],
        "properties": {
          "input_file": { "type": "string" },
          "output_dir": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        // TIP: Add your own style guide rules here.
        // What level of evidence do you require? Are anecdotes valid support?
        // How do you feel about "studies show" without a citation?
        "instructions": { "inline": "Identify claims that are logically unsupported.\n\n1. Read the full file at `input_file`.\n2. Extract every **claim** — any statement asserting something is true, should be done, or will happen. Ignore pure opinion flagged as such (\"I think...\"), definitions, and direct quotes.\n3. For each claim, check whether the piece provides support:\n   - **Evidence**: data, citation, concrete example\n   - **Logical argument**: reasoning from premises to conclusion\n   - **Authority**: citing a credible source (not \"experts say\")\n4. Flag claims with **no support**, and claims with **weak support**:\n   - **Circular**: restates the claim in different words\n   - **Non-sequitur**: the evidence doesn't support the conclusion\n   - **Weasel-worded**: \"many people believe\", \"it's well known that\"\n5. Write to `{output_dir}/unsupported-claims.md` using write_file:\n\n```\n# Unsupported Claims\n\n## Summary\nN claims found, M unsupported, K weakly supported\n\n## Unsupported Claims\n\n### 1. \"[quote the claim]\"\n- **Location**: paragraph N\n- **Issue**: no support / circular / non-sequitur / weasel words\n- **Suggestion**: what evidence would support this?\n\n## Weakly Supported Claims\n[Same format]\n```\n\nReturn `[]` when done." }
      },
      "next": []
    },
    {
      "name": "StructuralFlow",
      "value_schema": {
        "type": "object",
        "required": ["input_file", "output_dir"],
        "properties": {
          "input_file": { "type": "string" },
          "output_dir": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        // TIP: Add your own style guide rules here.
        // Do you prefer explicit transitions or implicit thematic links?
        // What's your tolerance for tangents?
        // Is repetition for emphasis okay, or always a flaw?
        "instructions": { "inline": "Analyze the structural flow and transitions.\n\n1. Read the full file at `input_file`.\n2. Break the piece into **logical sections** (by heading, paragraph cluster, or thematic shift). For each, note:\n   - What it's about (one sentence)\n   - Its role in the argument (introduces, supports, counters, illustrates, concludes)\n3. Check **transitions**: Does each section follow logically? Are there abrupt topic changes? Could sections be reordered without loss?\n4. Check for **structural problems**:\n   - **Dead ends**: ideas introduced and never revisited\n   - **Buried lede**: the most important point appears late or understated\n   - **Front-loading**: all substance in the first half, second half rehashes\n   - **Missing counterargument**: argues one side without acknowledging the strongest objection\n5. Write to `{output_dir}/structural-flow.md` using write_file:\n\n```\n# Structural Flow\n\n## Section Map\n| # | Section | Role | Transition |\n|---|---------|------|------------|\n| 1 | ...     | ...  | (opening)  |\n| 2 | ...     | ...  | smooth / abrupt / missing |\n\n## Structural Issues\n[Dead ends, buried ledes, front-loading, missing counterarguments]\n\n## Flow Rating\ntight / mostly coherent / disjointed\n\n## Recommendations\n[Concrete suggestions]\n```\n\nReturn `[]` when done." }
      },
      "next": []
    }
  ]
}
```

## Running

```bash
barnum run --config config.json \
  --entrypoint-value '{"input_file": "drafts/essay.md", "output_dir": "review-output"}'
```

## How it works

1. **FanOut** creates the output directory and dispatches all three analyses in parallel.
2. **Thesis** reads the first ~500 words, identifies the central argument, and evaluates clarity, specificity, and placement. Writes to `thesis.md`.
3. **UnsupportedClaims** reads the full text, extracts every claim, and flags any that lack evidence, use circular reasoning, or rely on weasel words. Writes to `unsupported-claims.md`.
4. **StructuralFlow** maps the section structure, checks transitions between sections, and flags dead ends, buried ledes, and missing counterarguments. Writes to `structural-flow.md`.

## Key points

- All three analyses run in parallel — they read the same input file and write to separate output files
- Each agent's instructions are self-contained with a specific output format
- **Add your own style guide rules** to each agent's instructions. The comments in the config show where to customize: what counts as a thesis, what level of evidence you require, how strict you are about transitions. These rules are what make the assistant useful for *your* writing, not generic writing
- Adding a new analysis (e.g., tone consistency, audience appropriateness) means adding one step and one entry in the FanOut dispatch. The rest stays unchanged
