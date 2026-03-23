---
image: /img/og/repertoire-document-verification.png
---

# Document Verification

Extract verifiable facts from a document, then research and evaluate each one independently. Results are written as individual verdict files to an output directory.

## The Pattern

```
                        ┌──→ EvaluateFact (001-revenue-growth) ──→ WriteFile
                        │
IdentifyFacts ──────────┼──→ EvaluateFact (002-market-share) ──→ WriteFile
                        │
                        └──→ EvaluateFact (003-patent-filing-date) ──→ WriteFile

Each WriteFile writes:
  output_dir/001-revenue-growth.true.txt
  output_dir/002-market-share.false.txt
  output_dir/003-patent-filing-date.unknown.txt
```

## Example: Fact-check a document

```jsonc
{
  "entrypoint": "IdentifyFacts",
  "steps": [
    {
      "name": "IdentifyFacts",
      "value_schema": {
        "type": "object",
        "required": ["file", "output_dir"],
        "properties": {
          "file": { "type": "string" },
          "output_dir": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "kind": "Inline", "value": "Read the document at the path in `file`.\n\nExtract every verifiable factual claim. A verifiable claim is one that can be confirmed or denied through research — dates, statistics, named events, attributed quotes, technical specifications, etc. Skip opinions, predictions, and subjective assessments.\n\nAssign each fact a numeric ID (starting at 1, zero-padded to 3 digits) and a short kebab-case label (no spaces, lowercase, max 5 words) describing the claim.\n\nReturn one EvaluateFact task per claim. Example: [{\"kind\": \"EvaluateFact\", \"value\": {\"id\": \"001-revenue-doubled-in-2024\", \"claim\": \"The company's revenue doubled from $50M to $100M in fiscal year 2024.\", \"source_file\": \"report.md\", \"output_dir\": \"output/\"}}]\n\nIf the document contains no verifiable claims, return []." }
      },
      "next": ["EvaluateFact"]
    },
    {
      "name": "EvaluateFact",
      "value_schema": {
        "type": "object",
        "required": ["id", "claim", "source_file", "output_dir"],
        "properties": {
          "id": { "type": "string" },
          "claim": { "type": "string" },
          "source_file": { "type": "string" },
          "output_dir": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "kind": "Inline", "value": "You are a fact-checker. Evaluate whether the following claim is true, false, or unknown.\n\nThe claim: see `claim` in the input.\nThe source document: see `source_file` (read it for context if needed).\n\nResearch the claim thoroughly:\n1. Look for primary sources, official records, or authoritative references that confirm or deny the claim.\n2. Check for common misquotations, outdated figures, or misleading framings.\n3. Consider whether the claim is technically true but misleading.\n\nReach a verdict: exactly one of \"true\", \"false\", or \"unknown\" (if evidence is insufficient or contradictory).\n\nReturn a WriteFile task. The path must be `{output_dir}/{id}.{verdict}.txt` where verdict is one of true, false, unknown. The content should include the original claim, your verdict, a summary of evidence for and against, and references consulted.\n\nExample: [{\"kind\": \"WriteFile\", \"value\": {\"path\": \"output/001-revenue-doubled-in-2024.true.txt\", \"content\": \"Claim: The company's revenue doubled...\\n\\nVerdict: true\\n\\nEvidence: SEC filings confirm...\"}}]" }
      },
      "next": ["WriteFile"]
    },
    {
      "name": "WriteFile",
      // Generic file-writing step. Agents return data; this step handles I/O.
      "value_schema": {
        "type": "object",
        "required": ["path", "content"],
        "properties": {
          "path": { "type": "string" },
          "content": { "type": "string" }
        }
      },
      "action": {
        "kind": "Command",
        "script": "INPUT=$(cat) && FILEPATH=$(echo \"$INPUT\" | jq -r '.value.path') && CONTENT=$(echo \"$INPUT\" | jq -r '.value.content') && mkdir -p \"$(dirname \"$FILEPATH\")\" && printf '%s\\n' \"$CONTENT\" > \"$FILEPATH\" && echo '[]'"
      },
      "next": []
    }
  ]
}
```

## Running

```js
import { barnumRun } from "@barnum/barnum";

barnumRun({
  config: "config.json",
  entrypointValue: '{"file": "claims.md", "output_dir": "verification-output"}',
}).on("exit", (code) => process.exit(code ?? 1));
```

## How it works

1. **IdentifyFacts** reads the document and extracts every verifiable factual claim. Each gets a unique ID like `001-revenue-doubled-in-2024`. Returns one `EvaluateFact` task per claim.
2. **EvaluateFact** runs in parallel for each claim. The agent researches the claim, reaches a verdict, and returns a `WriteFile` task with the path (encoding the verdict in the filename) and the full evaluation as content.
3. **WriteFile** is a generic Command step that writes any `{path, content}` pair to disk, creating directories as needed.
4. When all evaluations complete, the output directory contains one file per claim. The filename encodes the verdict (`*.true.txt`, `*.false.txt`, `*.unknown.txt`), making it trivial to scan results at a glance.

## Variant: Adversarial verification

For higher confidence, replace the single evaluation step with an adversarial debate. One agent argues the claim is true, another argues it's false, and a judge weighs both arguments.

Each fact gets its own `DebateFact` coordinator that fans out to both advocates and uses `finally` to trigger the judge only after both sides have submitted their arguments.

```
                        ┌──→ DebateFact (001-revenue-growth)
                        │
IdentifyFacts ──────────┼──→ DebateFact (002-market-share)
                        │
                        └──→ DebateFact (003-patent-filing-date)

Per fact:
┌──────────────────────────────────────────────────────────────┐
│  DebateFact (with finally)                                   │
│                                                              │
│  DebateFact ──┬──→ ArgueTrue ──→ WriteFile                   │
│               └──→ ArgueFalse ──→ WriteFile                  │
│                                                              │
│  ════════════════════════════════════════════════════════════ │
│  After BOTH advocates complete:                              │
│                                                              │
│  finally ──→ JudgeFact ──→ WriteFile                         │
└──────────────────────────────────────────────────────────────┘
```

```jsonc
{
  "entrypoint": "IdentifyFacts",
  "steps": [
    {
      "name": "IdentifyFacts",
      "value_schema": {
        "type": "object",
        "required": ["file", "output_dir"],
        "properties": {
          "file": { "type": "string" },
          "output_dir": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "kind": "Inline", "value": "Read the document at the path in `file`.\n\nExtract every verifiable factual claim. A verifiable claim is one that can be confirmed or denied through research — dates, statistics, named events, attributed quotes, technical specifications, etc. Skip opinions, predictions, and subjective assessments.\n\nAssign each fact a numeric ID (starting at 1, zero-padded to 3 digits) and a short kebab-case label (no spaces, lowercase, max 5 words) describing the claim.\n\nReturn one DebateFact task per claim. Example: [{\"kind\": \"DebateFact\", \"value\": {\"id\": \"001-revenue-doubled-in-2024\", \"claim\": \"The company's revenue doubled from $50M to $100M in fiscal year 2024.\", \"source_file\": \"report.md\", \"output_dir\": \"output/\"}}]\n\nIf the document contains no verifiable claims, return []." }
      },
      "next": ["DebateFact"]
    },
    {
      "name": "DebateFact",
      // Coordinator: fans out to both advocates, then triggers the judge via finally.
      "value_schema": {
        "type": "object",
        "required": ["id", "claim", "source_file", "output_dir"],
        "properties": {
          "id": { "type": "string" },
          "claim": { "type": "string" },
          "source_file": { "type": "string" },
          "output_dir": { "type": "string" }
        }
      },
      "action": {
        "kind": "Command",
        "script": "INPUT=$(cat) && V=$(echo \"$INPUT\" | jq -c '.value') && echo \"$V\" | jq -c '[{kind: \"ArgueTrue\", value: .}, {kind: \"ArgueFalse\", value: .}]'"
      },
      "finally": {
        "kind": "Command",
        "script": "INPUT=$(cat) && echo \"$INPUT\" | jq -c '[{kind: \"JudgeFact\", value: .}]'"
      },
      "next": ["ArgueTrue", "ArgueFalse"]
    },
    {
      "name": "ArgueTrue",
      "value_schema": {
        "type": "object",
        "required": ["id", "claim", "source_file", "output_dir"],
        "properties": {
          "id": { "type": "string" },
          "claim": { "type": "string" },
          "source_file": { "type": "string" },
          "output_dir": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "kind": "Inline", "value": "You are an advocate arguing that the following claim is TRUE.\n\nThe claim: see `claim` in the input.\nThe source document: see `source_file` (read it for context if needed).\n\nResearch the claim and build the strongest possible case that it is true:\n1. Find primary sources, official records, or authoritative references that support the claim.\n2. Address obvious counterarguments preemptively.\n3. If the claim is partially true, argue for the interpretation that makes it most accurate.\n\nReturn a WriteFile task with your argument. The path must be `{output_dir}/{id}.argue-true.txt`. The content should include: the original claim, your best evidence supporting it, sources consulted, and preemptive rebuttals to likely counterarguments.\n\nExample: [{\"kind\": \"WriteFile\", \"value\": {\"path\": \"output/001-revenue-doubled-in-2024.argue-true.txt\", \"content\": \"Claim: ...\\n\\nEvidence: ...\\n\\nSources: ...\"}}]" }
      },
      "next": ["WriteFile"]
    },
    {
      "name": "ArgueFalse",
      "value_schema": {
        "type": "object",
        "required": ["id", "claim", "source_file", "output_dir"],
        "properties": {
          "id": { "type": "string" },
          "claim": { "type": "string" },
          "source_file": { "type": "string" },
          "output_dir": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "kind": "Inline", "value": "You are an advocate arguing that the following claim is FALSE.\n\nThe claim: see `claim` in the input.\nThe source document: see `source_file` (read it for context if needed).\n\nResearch the claim and build the strongest possible case that it is false or misleading:\n1. Find primary sources, official records, or authoritative references that contradict the claim.\n2. Identify misleading framings, outdated figures, or missing context that make the claim deceptive even if technically true.\n3. Address obvious counterarguments preemptively.\n\nReturn a WriteFile task with your argument. The path must be `{output_dir}/{id}.argue-false.txt`. The content should include: the original claim, your best evidence against it, sources consulted, and preemptive rebuttals to likely counterarguments.\n\nExample: [{\"kind\": \"WriteFile\", \"value\": {\"path\": \"output/001-revenue-doubled-in-2024.argue-false.txt\", \"content\": \"Claim: ...\\n\\nEvidence: ...\\n\\nSources: ...\"}}]" }
      },
      "next": ["WriteFile"]
    },
    {
      "name": "JudgeFact",
      "value_schema": {
        "type": "object",
        "required": ["id", "claim", "source_file", "output_dir"],
        "properties": {
          "id": { "type": "string" },
          "claim": { "type": "string" },
          "source_file": { "type": "string" },
          "output_dir": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "kind": "Inline", "value": "You are an impartial judge evaluating a factual claim.\n\nThe claim: see `claim` in the input.\n\nTwo advocates have already researched this claim and written their arguments:\n- `{output_dir}/{id}.argue-true.txt` — the case that the claim is true\n- `{output_dir}/{id}.argue-false.txt` — the case that the claim is false\n\nRead both arguments carefully. Evaluate the quality of evidence on each side:\n1. Which side cites stronger, more authoritative sources?\n2. Which side's reasoning is more rigorous?\n3. Are there logical fallacies or unsupported leaps on either side?\n4. Is one side's evidence clearly more current and relevant?\n\nReach a verdict: exactly one of \"true\", \"false\", or \"unknown\" (if both sides present compelling evidence and you cannot determine the answer with confidence).\n\nReturn a WriteFile task. The path must be `{output_dir}/{id}.{verdict}.txt` where verdict is the one you chose. The content should include the original claim, your verdict, and your reasoning citing specific evidence from both arguments.\n\nExample: [{\"kind\": \"WriteFile\", \"value\": {\"path\": \"output/001-revenue-doubled-in-2024.true.txt\", \"content\": \"Claim: ...\\n\\nVerdict: true\\n\\nReasoning: The true-side cited SEC filings...\"}}]" }
      },
      "next": ["WriteFile"]
    },
    {
      "name": "WriteFile",
      // Generic file-writing step. Agents return data; this step handles I/O.
      "value_schema": {
        "type": "object",
        "required": ["path", "content"],
        "properties": {
          "path": { "type": "string" },
          "content": { "type": "string" }
        }
      },
      "action": {
        "kind": "Command",
        "script": "INPUT=$(cat) && FILEPATH=$(echo \"$INPUT\" | jq -r '.value.path') && CONTENT=$(echo \"$INPUT\" | jq -r '.value.content') && mkdir -p \"$(dirname \"$FILEPATH\")\" && printf '%s\\n' \"$CONTENT\" > \"$FILEPATH\" && echo '[]'"
      },
      "next": []
    }
  ]
}
```

### Running the adversarial variant

```js
import { barnumRun } from "@barnum/barnum";

barnumRun({
  config: "config.json",
  entrypointValue: '{"file": "claims.md", "output_dir": "verification-output"}',
}).on("exit", (code) => process.exit(code ?? 1));
```

### How the adversarial variant works

1. **IdentifyFacts** extracts claims and emits one `DebateFact` task per claim.
2. **DebateFact** is a coordinator. It fans out to `ArgueTrue` and `ArgueFalse` in parallel. Its `finally` hook fires only after both advocates complete, dispatching `JudgeFact`.
3. **ArgueTrue** researches the claim and returns a `WriteFile` task with its argument as data.
4. **ArgueFalse** researches the claim and returns a `WriteFile` task with its argument as data.
5. **WriteFile** is a generic Command step that writes any `{path, content}` pair to disk, creating directories as needed.
6. **JudgeFact** reads both argument files (written by the advocates via `WriteFile`), weighs the evidence, and returns a `WriteFile` task with the final verdict. The verdict (`true`, `false`, or `unknown`) is encoded in the filename.

No agent writes to the filesystem directly. All file I/O flows through the `WriteFile` Command step.

The adversarial structure forces thorough research on both sides. A single evaluator might confirm its first impression; two opposing advocates are forced to find the strongest evidence for their assigned position, giving the judge better raw material.

## Key points

- Fact IDs use the format `{NNN}-{kebab-case-label}`, making output files sort naturally and scan easily
- The simple variant uses one agent per fact; the adversarial variant uses three (argue true, argue false, judge) for higher confidence
- The `DebateFact` coordinator with `finally` ensures the judge only runs after both advocates finish — not before, not after all facts globally
- Verdict is encoded in the filename (`*.true.txt`, `*.false.txt`, `*.unknown.txt`), so you can count results with `ls *.true.txt | wc -l`
- No agent writes to disk directly — all file I/O flows through the generic `WriteFile` Command step. Agents return data; infrastructure handles I/O
- In the adversarial variant, advocates communicate findings to the judge via the filesystem. `WriteFile` writes argument files, and `JudgeFact` reads them. A future version of Barnum will provide a direct mechanism for passing data between sibling tasks, eliminating this workaround
