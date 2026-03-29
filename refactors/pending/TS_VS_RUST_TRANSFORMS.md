# Where Should AST Transformations Live?

A thought piece, not an action item.

## The question

Barnum has two places where AST structure can be created or rewritten:

1. **TypeScript (definition time)**: The TS combinator functions (`pipe`, `augment`, `tap`, `withResource`, etc.) construct AST nodes at the time the user defines a workflow. The Rust scheduler never sees the original high-level intent — it sees the expanded result.

2. **Rust (execution time)**: The scheduler interprets AST nodes directly. If a concept has its own AST node, the scheduler handles it with purpose-built logic, full visibility, and dedicated error messages.

Every combinator in Barnum implicitly makes this choice. Some are clearly in one camp, some could go either way.

## Current state

### Transforms that happen in TypeScript

| Combinator | Expands to | Why it's TS-side |
|---|---|---|
| `pipe(a, b, c)` | `Chain(a, Chain(b, c))` | Syntactic sugar for nested Chain. Trivial expansion. |
| `augment(action)` | `Chain(Parallel(action, Identity), Merge)` | Convenience pattern built from existing primitives. |
| `tap(action)` | `Chain(Parallel(Chain(action, Constant({})), Identity), Merge)` | Same — composition of existing primitives. |
| `withResource(...)` | ~30 lines of Chain/Parallel/ExtractIndex assembly | Resource lifecycle pattern. |
| `dropResult(action)` | `Chain(action, Drop)` | Trivial two-node chain. |
| `range(start, end)` | `Constant([start, ..., end-1])` | Computed at definition time, emitted as constant. |

### Concepts with native AST nodes

| Concept | AST node | Why it's Rust-side |
|---|---|---|
| Sequential composition | `Chain` | Fundamental. Can't be expressed more simply. |
| Fan-out | `Parallel` | Fundamental. |
| Array mapping | `ForEach` | Could be expressed as "extract length, range, parallel-map" but that's absurd. ForEach has clear semantics the scheduler needs to understand. |
| Branching | `Branch` | Could technically be a chain of if-else pairs, but the scheduler needs the full case map for error messages ("no matching case for kind X") and potentially future exhaustiveness checking. |
| Iteration | `Loop` | Could be expressed with `scope`/continuation (see LET_BINDINGS.md), but the Continue/Break protocol has specific error semantics. |
| Named step reference | `Step` | The scheduler resolves step targets during flattening. Fundamental to the execution model. |
| Scoped bindings | `Declare` (planned) | Chose native node over TS-side closure conversion. See LET_BINDINGS.md. |

## Tradeoffs

### Arguments for TS-side transforms

**Iteration speed.** Adding a TS combinator is a single function in `builtins.ts`. No Rust changes, no schema regeneration, no flattener updates, no engine changes, no snapshot updates. The entire feedback loop is `pnpm test`. This is the single biggest practical advantage.

**Smaller AST grammar.** Fewer node types means fewer match arms everywhere — flattener, engine `advance`, engine `deliver`, serialization, schema generation. Every new AST node is O(N) code additions across M consumers. TS-side transforms keep the grammar small.

**Composability.** TS-side transforms compose freely. `augment` uses `parallel` and `merge`. `tap` uses `augment`. `withResource` uses `augment`, `extractIndex`, and `chain`. These compositions just work because they produce standard AST nodes. There's no need for the scheduler to understand the higher-level pattern — it executes the primitives.

**TypeScript is expressive.** Pattern construction in TypeScript is straightforward — you're building an object graph. In Rust, the same construction would be runtime AST manipulation, which is more cumbersome and would duplicate the type safety that TypeScript provides at definition time.

### Arguments for Rust-side (native AST nodes)

**Error messages.** When something fails inside a `withResource`, the error references synthetic `Parallel`/`ExtractIndex` nodes that the user never wrote. If `withResource` were a native node, the error would say "resource disposal failed in withResource" instead of "handler at parallel branch 1, chain position 2 failed." This is the strongest argument for native nodes.

**Debugging and visualization.** If you ever build a workflow visualizer, TS-expanded nodes are opaque. A `tap` looks like a Parallel-Chain-Constant-Identity-Merge tree. A native `Tap` node would render as a single labeled box. The scheduler's view of the AST doesn't match the user's intent.

**Optimization.** The scheduler can optimize native nodes. A `Declare` node lets the scheduler cache binding values and manage their lifetimes. A TS-side closure conversion threads values through the pipeline, and the scheduler can't tell which Parallel nodes are "real concurrency" vs. "just env-threading." Similarly, a native `Tap` node could be optimized to "fire and forget" without waiting for the side effect — impossible if the scheduler only sees the Parallel-Merge expansion.

**Correctness under future changes.** A TS-side expansion bakes in assumptions about the primitives it composes. If the semantics of `Parallel` change (e.g., error propagation policy, cancellation), every TS-side transform that uses `Parallel` inherits the new semantics — which may not be correct for that transform. A native `Tap` node would have its own error/cancellation policy, independent of Parallel's.

**Feature interactions.** When features are TS-side expansions, they interact through their expanded forms. If `tap` and `declare` both expand to Parallel-based ASTs, the `declare` closure conversion transform would need to handle `tap`'s synthetic Parallel nodes. Native nodes avoid this quadratic interaction problem (see LET_BINDINGS.md, Approach B rejection reasoning).

## The spectrum

Not every combinator deserves a native AST node. There's a spectrum:

### Clearly TS-side

- **`pipe`**: Pure syntactic sugar. `Chain(a, Chain(b, c))` IS the meaning. There's no semantic loss.
- **`dropResult`**: `Chain(action, Drop)`. Two nodes. Trivial.
- **`range`**: Computed at definition time. The scheduler never needs to know this was a range.

These add no semantic information that the scheduler could use. Making them native nodes would add complexity for zero benefit.

### Clearly Rust-side

- **`ForEach`**: The scheduler needs to know "this is an array iteration" to dispatch per-element, collect results in order, and report errors per-element. Expanding to manual indexing/parallel would lose all of this.
- **`Branch`**: Error messages ("no matching case") require the scheduler to see the full case map. Future exhaustiveness checking requires it too.
- **`Declare`**: The environment is a runtime concept the scheduler manages. TS-side closure conversion threads env through every node, hiding the semantic intent and making feature interactions quadratic.

### The gray area

This is the interesting part:

**`augment`** — Currently TS-side (`Parallel(action, Identity) → Merge`). It's a common pattern, and its expansion is only 3 nodes. Error messages say "merge failed" instead of "augment merge step failed" but the difference is marginal. A native node might help with visualization. **Verdict: probably fine as TS-side.** The expansion is short and the semantics are transparent.

**`tap`** — Currently TS-side, 5-node expansion. A native `Tap` could be optimized (fire-and-forget side effects) and would produce better error messages. But `tap` is fundamentally "run and discard," which the existing primitives express adequately. **Verdict: TS-side for now.** If we add cancellation/timeout semantics, revisit.

**`withResource`** — Currently TS-side, ~30 lines of assembly. This is the most complex TS-side transform and the one most likely to benefit from a native node. It doesn't handle cleanup-on-failure (the expansion would need try/finally-equivalent restructuring). Error messages are inscrutable. A native `WithResource` node would let the scheduler handle the lifecycle with purpose-built logic, including proper error-path cleanup. **Verdict: strong candidate for promotion to native node.** The TS-side expansion is already straining.

**`loop`** — Currently a native node. Could theoretically be expressed as `scope("loop", (restart) => body → branch(Continue: restart, Break: identity))` if we had a general scope/continuation primitive. But Loop has specific error messages ("loop body must return Continue/Break") that a general scope wouldn't provide. **Verdict: keep as native node.** Even if we add `scope`, Loop should remain as a semantic wrapper with its own error messages.

## The decision heuristic

When deciding where a new combinator belongs:

1. **Does the scheduler need to see it for correctness?** If the combinator has runtime behavior that can't be expressed by composing existing nodes (ForEach's per-element dispatch, Branch's case matching, Declare's environment), it must be a native node.

2. **Is the expansion more than ~5 nodes?** Short expansions (pipe, augment, dropResult) are readable in debug output. Long expansions (withResource at ~15 nodes) are not. Longer expansions are candidates for native nodes.

3. **Does the combinator have error semantics?** If a failure inside the combinator should produce a specific, meaningful error message ("loop body must return Continue/Break"), it needs a native node. Generic "parallel branch 2 failed" messages from an expansion are not acceptable.

4. **Will future features interact with it?** If other combinators will need to "see through" this one (like Declare's closure conversion needing to handle every node type), keeping it as a native node avoids quadratic interaction costs.

5. **Does the combinator have optimization potential?** If the scheduler could do something smarter with semantic knowledge (cache Declare bindings, fire-and-forget Tap, cleanup-on-error for WithResource), a native node enables that.

If the answer to all five is "no," it's a TS-side combinator.

## The practical reality

Right now, TS-side transforms are the default because they're cheap to add. This is correct for the current stage of the project. The cost of a native AST node is real:

- Rust AST struct + enum variant
- Flat representation variant + flatten logic
- Engine advance + deliver match arms
- Frame kind variant (if structural)
- Snapshot test updates
- Schema regeneration

That's 6+ files for every new node. TS-side is 1 file (builtins.ts). The economics strongly favor TS-side for anything that doesn't clearly need scheduler visibility.

As the project matures and error reporting, debugging, and optimization become more important, some TS-side combinators may get promoted to native nodes. The candidates, in rough order of likelihood:

1. **`withResource`** — already straining, needs error-path cleanup
2. **`tap`** — if cancellation/timeout semantics are added
3. **`augment`** — probably never; the expansion is too simple

This isn't a problem to solve now. It's a dimension to be aware of as the AST grammar evolves.

## Aside: a third option

There's a middle ground we haven't used: **Rust-side transforms during flattening.** The flattener could recognize patterns in the tree AST and rewrite them before producing the flat representation. For example, the flattener could detect the `Parallel(action, Identity) → Merge` pattern and tag the resulting flat nodes with metadata: "this parallel is an augment."

This gives the scheduler semantic information without adding new AST nodes to the TS → Rust serialization boundary. The TS side emits the expansion (cheap to add), and the Rust flattener recovers the semantic intent (cheap to consume).

We don't need this yet, but it's worth noting as a future escape hatch. It's how many compilers work: the source language has a large set of surface constructs, the IR has a small set of primitives, and optimization passes recognize patterns in the IR to recover high-level intent.
