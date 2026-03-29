# Step Definition Design

How should named steps (for mutual recursion, DAG topologies, and reuse) be defined?

## Current Design: Separate HashMap on Config

```json
{
  "workflow": { "kind": "Step", "step": "Main" },
  "steps": {
    "Main": { "kind": "Sequence", "actions": [...] },
    "Process": { "kind": "Call", "handler": {...} }
  }
}
```

**Pros:**
- Simple flat structure. All steps are peers.
- Easy to validate: check that every `Step` reference resolves to a key in the map.
- Natural for serialization — no recursive binding syntax needed in JSON.
- Mirrors how Temporal and Step Functions define activities/states (flat registry).

**Cons:**
- Steps are stringly-typed references. Typos in step names are runtime errors, not type errors.
- No lexical scoping — every step is globally visible. Can't have local helper steps.
- Separation between `workflow` and `steps` is awkward. Why is the entry point special?

## Alternative: Let-Rec (Recursive Let Bindings)

```json
{
  "kind": "LetRec",
  "bindings": {
    "Process": { "kind": "Call", "handler": {...} }
  },
  "body": { "kind": "Sequence", "actions": [
    { "kind": "Ref", "name": "Process" },
    ...
  ]}
}
```

**Pros:**
- Steps are scoped. A `LetRec` node can appear anywhere in the AST, not just at the top level.
- Enables local helper steps that don't pollute the global namespace.
- `LetRec` is a well-understood construct from PL theory (fixed-point combinator).
- Config becomes just `{ workflow: Action }` — no separate `steps` field.

**Cons:**
- More complex AST. `LetRec` is a new node type with bindings + body.
- JSON representation is heavier for the common case (most workflows are flat).
- Harder to validate statically — need to track scoping rules.
- Over-engineering for the current use case (flat step registries are sufficient).

## Alternative: Inline Definitions

Steps are defined inline where first referenced, with subsequent references being lookups.

**Rejected:** This conflates definition with use and makes the AST order-dependent.

## Open Questions

1. Should `Config.workflow` just be a step name (string) that indexes into `steps`? That removes the asymmetry of having a distinguished entry point.
2. Do we need scoped steps at all, or is a flat registry sufficient for all foreseeable use cases?
3. On the TypeScript side, can we make step references type-safe (e.g., generic `step<K extends keyof Steps>(name: K)`) so typos are caught at compile time?
