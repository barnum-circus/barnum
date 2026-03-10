# Legal review

Fan out a contract review into parallel specialist analyses, then synthesize a final recommendation.

## Why This Pattern?

Legal contracts require expertise across multiple domains. Rather than asking one agent to catch everything, fan out to specialists that each focus on a single concern — then aggregate their findings into one report.

## The Pattern

```
┌──────────────────────────────────────────────────────────────┐
│  AnalyzeContract (with finally)                              │
│                                                              │
│  AnalyzeContract ──┬──→ CourtCaseReferences                  │
│                    ├──→ FinancialClaims                       │
│                    └──→ LiabilityAnalysis                     │
│                                                              │
│  ═══════════════════════════════════════════════════════════  │
│  After ALL descendants complete:                             │
│                                                              │
│  finally ──→ SynthesizeReview ──→ Done                       │
└──────────────────────────────────────────────────────────────┘
```

## Example: Contract due diligence

```jsonc
{
  "entrypoint": "AnalyzeContract",
  "steps": [
    {
      "name": "AnalyzeContract",
      "value_schema": {
        "type": "object",
        "required": ["contract_path", "output_dir"],
        "properties": {
          "contract_path": { "type": "string" },
          "output_dir": { "type": "string" }
        }
      },
      "action": {
        "kind": "Command",
        // Dispatch three parallel analyses for the same contract.
        "script": "CONTRACT=$(jq -r '.value.contract_path') && OUT=$(jq -r '.value.output_dir') && mkdir -p \"$OUT\" && echo \"[{\\\"kind\\\": \\\"CourtCaseReferences\\\", \\\"value\\\": {\\\"contract_path\\\": \\\"$CONTRACT\\\", \\\"output_dir\\\": \\\"$OUT\\\"}}, {\\\"kind\\\": \\\"FinancialClaims\\\", \\\"value\\\": {\\\"contract_path\\\": \\\"$CONTRACT\\\", \\\"output_dir\\\": \\\"$OUT\\\"}}, {\\\"kind\\\": \\\"LiabilityAnalysis\\\", \\\"value\\\": {\\\"contract_path\\\": \\\"$CONTRACT\\\", \\\"output_dir\\\": \\\"$OUT\\\"}}]\""
      },
      // After all three analyses complete, synthesize the findings.
      "finally": "jq -r '.value | \"[{\\\"kind\\\": \\\"SynthesizeReview\\\", \\\"value\\\": {\\\"output_dir\\\": \\\"\" + .output_dir + \"\\\"}}]\"'",
      "next": ["CourtCaseReferences", "FinancialClaims", "LiabilityAnalysis"]
    },
    {
      "name": "CourtCaseReferences",
      "value_schema": {
        "type": "object",
        "required": ["contract_path", "output_dir"],
        "properties": {
          "contract_path": { "type": "string" },
          "output_dir": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "inline": "You are a legal analyst specializing in case law.\n\n1. Read the contract at the path in `contract_path`.\n2. Identify every reference to court cases, legal precedents, and statutory citations.\n3. For each reference, look up the actual case or statute. Summarize the ruling and assess whether the contract's citation is accurate and used in proper context.\n4. Flag any references that are misleading, taken out of context, or cite overturned decisions.\n\nWrite your findings to `{output_dir}/court_case_references.md` with a section per citation.\n\nReturn `[]` when done." }
      },
      "next": []
    },
    {
      "name": "FinancialClaims",
      "value_schema": {
        "type": "object",
        "required": ["contract_path", "output_dir"],
        "properties": {
          "contract_path": { "type": "string" },
          "output_dir": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "inline": "You are a financial analyst reviewing contract terms.\n\n1. Read the contract at the path in `contract_path`.\n2. Identify all financial claims: payment terms, penalty clauses, interest rates, cap amounts, royalty percentages, and revenue-sharing formulas.\n3. For each claim, verify that the numbers are internally consistent (e.g., percentages add up, caps are reasonable relative to contract value).\n4. Flag any terms that are unusual, one-sided, or significantly above/below market rates.\n5. Cross-reference any financial projections or estimates against the assumptions stated in the contract.\n\nWrite your findings to `{output_dir}/financial_claims.md`.\n\nReturn `[]` when done." }
      },
      "next": []
    },
    {
      "name": "LiabilityAnalysis",
      "value_schema": {
        "type": "object",
        "required": ["contract_path", "output_dir"],
        "properties": {
          "contract_path": { "type": "string" },
          "output_dir": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "inline": "You are a legal analyst specializing in liability and risk.\n\n1. Read the contract at the path in `contract_path`.\n2. Identify all indemnification clauses, limitation of liability provisions, warranty disclaimers, and force majeure terms.\n3. Assess whether the liability allocation is balanced or heavily favors one party.\n4. Flag any unlimited liability exposure, broad indemnification obligations, or missing standard protections (e.g., no liability cap, no mutual indemnification).\n5. Note any clauses that could create unexpected obligations under adverse conditions.\n\nWrite your findings to `{output_dir}/liability_analysis.md`.\n\nReturn `[]` when done." }
      },
      "next": []
    },
    {
      "name": "SynthesizeReview",
      "value_schema": {
        "type": "object",
        "required": ["output_dir"],
        "properties": {
          "output_dir": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "inline": "You are a senior legal reviewer producing a final recommendation.\n\n1. Read all analysis files in `output_dir`:\n   - `court_case_references.md`\n   - `financial_claims.md`\n   - `liability_analysis.md`\n2. Synthesize the findings into a single executive summary.\n3. Categorize issues by severity: **Critical** (deal-breakers), **Major** (require negotiation), **Minor** (acceptable risk).\n4. Produce a clear **recommendation**: sign as-is, sign with amendments, or do not sign.\n5. If amendments are recommended, list the specific clauses that need changes and what the changes should be.\n\nWrite the final report to `{output_dir}/recommendation.md`.\n\nReturn `[]` when done." }
      },
      "next": []
    }
  ]
}
```

## Running

```bash
gsd run \
  --config config.json \
  --pool agents \
  --entrypoint-value '{"contract_path": "contracts/vendor-agreement.pdf", "output_dir": "review-output"}'
```

## How it works

1. **AnalyzeContract** creates the output directory and dispatches three parallel analysis tasks.
2. **CourtCaseReferences** reads the contract, finds every legal citation, looks up the actual cases, and flags inaccurate or misleading references. Writes to `court_case_references.md`.
3. **FinancialClaims** identifies all financial terms, checks them for internal consistency and market reasonableness. Writes to `financial_claims.md`.
4. **LiabilityAnalysis** examines indemnification, liability caps, and risk allocation. Writes to `liability_analysis.md`.
5. All three run in parallel and write to the shared output directory.
6. **finally** fires after all three complete, dispatching **SynthesizeReview**.
7. **SynthesizeReview** reads every analysis file, categorizes issues by severity, and writes a final sign/don't-sign recommendation to `recommendation.md`.

## Key points

- Each analyst writes to a file — the shared `output_dir` acts as a coordination mechanism between steps
- The `finally` hook ensures synthesis only happens after all analyses complete
- Adding a new concern (e.g., intellectual property review) means adding one step and one entry in the `next` array and command script — the rest of the workflow stays unchanged
- Each agent's instructions are self-contained: they know exactly what to look for and where to write
