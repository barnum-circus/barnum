# Document Verification

Extract verifiable facts from a document, then research and evaluate each one independently. Results are written as individual verdict files to an output directory.

## The Pattern

```
                        ┌──→ EvaluateFact (001-revenue-growth)
                        │
IdentifyFacts ──────────┼──→ EvaluateFact (002-market-share)
                        │
                        └──→ EvaluateFact (003-patent-filing-date)

Each EvaluateFact writes:
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
        "instructions": { "inline": "Read the document at the path in `file`.\n\nExtract every verifiable factual claim. A verifiable claim is one that can be confirmed or denied through research — dates, statistics, named events, attributed quotes, technical specifications, etc. Skip opinions, predictions, and subjective assessments.\n\nAssign each fact a numeric ID (starting at 1, zero-padded to 3 digits) and a short kebab-case label (no spaces, lowercase, max 5 words) describing the claim.\n\nReturn one EvaluateFact task per claim:\n```json\n[{\"kind\": \"EvaluateFact\", \"value\": {\"id\": \"001-revenue-doubled-in-2024\", \"claim\": \"The company's revenue doubled from $50M to $100M in fiscal year 2024.\", \"source_file\": \"report.md\", \"output_dir\": \"output/\"}}, ...]\n```\n\nIf the document contains no verifiable claims, return `[]`." }
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
        "instructions": { "inline": "You are a fact-checker. Your job is to evaluate whether the following claim is true, false, or unknown.\n\nThe claim: see `claim` in the input.\nThe source document: see `source_file` (read it for context if needed).\n\nResearch the claim thoroughly:\n1. Look for primary sources, official records, or authoritative references that confirm or deny the claim.\n2. Check for common misquotations, outdated figures, or misleading framings.\n3. Consider whether the claim is technically true but misleading.\n\nReach a verdict: **true**, **false**, or **unknown** (if evidence is insufficient or contradictory).\n\nWrite your findings to `{output_dir}/{id}.{verdict}.txt` where `{verdict}` is one of `true`, `false`, or `unknown`. Create the output directory if it doesn't exist.\n\nThe file should contain:\n- The original claim\n- Your verdict\n- A summary of the evidence for and against\n- Links or references to sources consulted\n\nReturn `[]` when done." }
      },
      "next": []
    }
  ]
}
```

## Running

```bash
barnum run --config config.json \
  --entrypoint-value '{"file": "claims.md", "output_dir": "verification-output"}'
```

## How it works

1. **IdentifyFacts** reads the document and extracts every verifiable factual claim. Each gets a unique ID like `001-revenue-doubled-in-2024`. Returns one `EvaluateFact` task per claim.
2. **EvaluateFact** runs in parallel for each claim. The agent researches the claim, reaches a verdict, and writes a file like `001-revenue-doubled-in-2024.true.txt` to the output directory.
3. When all evaluations complete, the output directory contains one file per claim. The filename encodes the verdict, making it trivial to scan results at a glance.

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
│  DebateFact ──┬──→ ArgueTrue                                 │
│               └──→ ArgueFalse                                │
│                                                              │
│  ════════════════════════════════════════════════════════════ │
│  After BOTH advocates complete:                              │
│                                                              │
│  finally ──→ JudgeFact ──→ WriteFact                         │
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
        "instructions": { "inline": "Read the document at the path in `file`.\n\nExtract every verifiable factual claim. A verifiable claim is one that can be confirmed or denied through research — dates, statistics, named events, attributed quotes, technical specifications, etc. Skip opinions, predictions, and subjective assessments.\n\nAssign each fact a numeric ID (starting at 1, zero-padded to 3 digits) and a short kebab-case label (no spaces, lowercase, max 5 words) describing the claim.\n\nReturn one DebateFact task per claim:\n```json\n[{\"kind\": \"DebateFact\", \"value\": {\"id\": \"001-revenue-doubled-in-2024\", \"claim\": \"The company's revenue doubled from $50M to $100M in fiscal year 2024.\", \"source_file\": \"report.md\", \"output_dir\": \"output/\"}}, ...]\n```\n\nIf the document contains no verifiable claims, return `[]`." }
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
        "script": "INPUT=$(cat) && echo \"$INPUT\" | jq -c '[{kind: \"JudgeFact\", value: .value}]'"
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
        "instructions": { "inline": "You are an advocate arguing that the following claim is TRUE.\n\nThe claim: see `claim` in the input.\nThe source document: see `source_file` (read it for context if needed).\n\nResearch the claim and build the strongest possible case that it is true:\n1. Find primary sources, official records, or authoritative references that support the claim.\n2. Address obvious counterarguments preemptively.\n3. If the claim is partially true, argue for the interpretation that makes it most accurate.\n\nWrite your argument to `{output_dir}/{id}.argue-true.txt`. Create the output directory if it doesn't exist.\n\nThe file should contain:\n- The original claim\n- Your best evidence supporting it\n- Sources consulted\n- Preemptive rebuttals to likely counterarguments\n\nReturn `[]` when done." }
      },
      "next": []
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
        "instructions": { "inline": "You are an advocate arguing that the following claim is FALSE.\n\nThe claim: see `claim` in the input.\nThe source document: see `source_file` (read it for context if needed).\n\nResearch the claim and build the strongest possible case that it is false or misleading:\n1. Find primary sources, official records, or authoritative references that contradict the claim.\n2. Identify misleading framings, outdated figures, or missing context that make the claim deceptive even if technically true.\n3. Address obvious counterarguments preemptively.\n\nWrite your argument to `{output_dir}/{id}.argue-false.txt`. Create the output directory if it doesn't exist.\n\nThe file should contain:\n- The original claim\n- Your best evidence against it\n- Sources consulted\n- Preemptive rebuttals to likely counterarguments\n\nReturn `[]` when done." }
      },
      "next": []
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
        "instructions": { "inline": "You are an impartial judge evaluating a factual claim.\n\nThe claim: see `claim` in the input.\n\nTwo advocates have already researched this claim and written their arguments:\n- `{output_dir}/{id}.argue-true.txt` — the case that the claim is true\n- `{output_dir}/{id}.argue-false.txt` — the case that the claim is false\n\nRead both arguments carefully. Evaluate the quality of evidence on each side:\n1. Which side cites stronger, more authoritative sources?\n2. Which side's reasoning is more rigorous?\n3. Are there logical fallacies or unsupported leaps on either side?\n4. Is one side's evidence clearly more current and relevant?\n\nReach a verdict: **true**, **false**, or **unknown** (if both sides present compelling evidence and you cannot determine the answer with confidence).\n\nReturn one WriteFact task:\n```json\n[{\"kind\": \"WriteFact\", \"value\": {\"id\": \"001-revenue-doubled-in-2024\", \"claim\": \"...\", \"verdict\": \"true\", \"reasoning\": \"The true-side cited SEC filings showing...\", \"output_dir\": \"output/\"}}]\n```" }
      },
      "next": ["WriteFact"]
    },
    {
      "name": "WriteFact",
      "value_schema": {
        "type": "object",
        "required": ["id", "claim", "verdict", "reasoning", "output_dir"],
        "properties": {
          "id": { "type": "string" },
          "claim": { "type": "string" },
          "verdict": { "type": "string" },
          "reasoning": { "type": "string" },
          "output_dir": { "type": "string" }
        }
      },
      "action": {
        "kind": "Command",
        "script": "INPUT=$(cat) && ID=$(echo \"$INPUT\" | jq -r '.value.id') && VERDICT=$(echo \"$INPUT\" | jq -r '.value.verdict') && CLAIM=$(echo \"$INPUT\" | jq -r '.value.claim') && REASONING=$(echo \"$INPUT\" | jq -r '.value.reasoning') && OUT=$(echo \"$INPUT\" | jq -r '.value.output_dir') && mkdir -p \"$OUT\" && printf 'Claim: %s\\n\\nVerdict: %s\\n\\nReasoning:\\n%s\\n' \"$CLAIM\" \"$VERDICT\" \"$REASONING\" > \"$OUT/$ID.$VERDICT.txt\" && echo '[]'"
      },
      "next": []
    }
  ]
}
```

### Running the adversarial variant

```bash
barnum run --config config.json \
  --entrypoint-value '{"file": "claims.md", "output_dir": "verification-output"}'
```

### How the adversarial variant works

1. **IdentifyFacts** extracts claims and emits one `DebateFact` task per claim.
2. **DebateFact** is a coordinator. It fans out to `ArgueTrue` and `ArgueFalse` in parallel. Its `finally` hook fires only after both advocates complete, dispatching `JudgeFact`.
3. **ArgueTrue** builds the strongest case that the claim is true and writes to `{id}.argue-true.txt`.
4. **ArgueFalse** builds the strongest case that the claim is false and writes to `{id}.argue-false.txt`.
5. **JudgeFact** reads both argument files, weighs the evidence, and returns a `WriteFact` task with a verdict.
6. **WriteFact** writes the final verdict file (e.g., `001-revenue-doubled-in-2024.true.txt`).

The adversarial structure forces thorough research on both sides. A single evaluator might confirm its first impression; two opposing advocates are forced to find the strongest evidence for their assigned position, giving the judge better raw material.

## Key points

- Fact IDs use the format `{NNN}-{kebab-case-label}`, making output files sort naturally and scan easily
- The simple variant uses one agent per fact; the adversarial variant uses three (argue true, argue false, judge) for higher confidence
- The `DebateFact` coordinator with `finally` ensures the judge only runs after both advocates finish — not before, not after all facts globally
- Verdict is encoded in the filename (`*.true.txt`, `*.false.txt`, `*.unknown.txt`), so you can count results with `ls *.true.txt | wc -l`
- The output directory is created on demand — no setup required
