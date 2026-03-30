# AST Compilation and Optimization

Ideas for optimizing the workflow AST before execution. The AST as authored is optimized for readability and composability; compilation transforms it for execution efficiency.

## Language-aware coalescing

Builtins exist in every language (Rust-native, TypeScript, future languages). When consecutive actions in a pipe share the same execution language, they can be coalesced into a single dispatch:

```
// Before: 3 separate Invoke dispatches
Pipe([
  Invoke(TS, "setup"),
  Invoke(TS, "process"),
  Invoke(TS, "check"),
])

// After: 1 batched TS dispatch
Invoke(TS, BatchedPipe(["setup", "process", "check"]))
```

This eliminates the per-step overhead of crossing the Rust↔TypeScript boundary (process spawn, JSON serialization, stdout capture) for consecutive same-language calls.

### Builtin placement heuristic

Builtins (identity, merge, extractField, tag, etc.) can execute in any language. The compiler should choose based on context:

- **At the start of a pipe**: prefer Rust (host language). The engine is already in Rust; executing the builtin natively avoids an unnecessary language boundary crossing.
- **Between two same-language steps**: prefer that language. If the preceding and following steps are both TypeScript, executing the builtin in TypeScript avoids two boundary crossings (Rust→TS→Rust→TS) in favor of zero (stays in TS).
- **Between two different-language steps**: prefer the host language (Rust), since a boundary crossing is unavoidable regardless.

General principle: **minimize boundary crossings**. Builtins are free to move between languages because their semantics are language-independent.

## Pipe flattening

Nested pipes are semantically equivalent to a single flat pipe:

```
// Before
Pipe([a, Pipe([b, c]), d])

// After
Pipe([a, b, c, d])
```

This is a straightforward structural simplification. Flattening happens before language-aware coalescing so that the coalescer sees the maximum possible run of same-language steps.

## Single-element container elimination

Containers with a single element can be unwrapped:

```
Pipe([a])       → a
All([a])        → a  (output still wrapped in array? depends on semantics)
ForEach(a)      → a  (when input is known to be a single-element array?)
```

`Pipe([a])` is always safe to simplify. `All([a])` requires care: if the output type wraps results in a tuple, removing the container changes the type. This optimization may only be valid when the type system can prove it's safe.

## Dead branch elimination

If static analysis can determine that a branch case is unreachable (e.g., the input type is a unit variant), that case can be removed. Unlikely to be useful in practice since branch inputs are usually runtime-discriminated.

## Step inlining

For steps referenced only once, the Step reference can be replaced with the step body inline. This eliminates the indirection of step lookup at execution time. Steps referenced multiple times (including recursively) must remain as references.

## Constant folding

If a pipe starts with `constant(v)` followed by a chain of builtins, the builtins can be evaluated at compile time and the entire sub-pipe replaced with a single `constant(result)`.

```
// Before
Pipe([constant({x: 1}), extractField("x"), tag("Num")])

// After
constant({kind: "Num", value: 1})
```

## Future: parallel scheduling hints

The compiler could analyze data dependencies to determine which branches of an all can start immediately vs. which depend on shared setup. This is more of an execution planner than a compilation pass.
