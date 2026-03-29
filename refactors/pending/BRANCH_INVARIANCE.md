# Branch Types Under Invariance

## Problem

With invariant `Pipeable`, zero-argument generic builtins (`drop`, `recur`, `done`) require explicit type parameters in branch cases. This is ergonomically bad — these functions don't semantically care about their input types.

### Current state

```ts
// drop() needs <HasErrors> to feed BranchInput computation
branch({
  HasErrors: pipe(extractField<HasErrors, "errors">("errors"), forEach(fix), recur<any>()),
  Clean: done<Clean>(),
})
```

`BranchInput<TCases>` computes the branch's input type by intersecting each case handler's input with `{ kind: K }`:

```ts
// libs/barnum/src/ast.ts
type BranchInput<TCases> = {
  [K in keyof TCases & string]: { kind: K } & ExtractInput<TCases[K]>;
}[keyof TCases & string];
```

If a case handler says `unknown` (e.g., bare `drop()`), the intersection `{ kind: K } & unknown` = `{ kind: K }` — missing the payload fields. Then invariance rejects the mismatch between `ClassifyResult` (which has `errors` on the HasErrors member) and `{ kind: "HasErrors" } | { kind: "Clean" }`.

So `drop<HasErrors>()` exists solely to smuggle type information into `BranchInput` that drop itself never uses.

### Affected builtins

All are zero-argument generic functions where TS can't infer the type parameter from arguments:

- **`drop<T>()`** — discards input. Type param is purely for BranchInput.
- **`recur<T>()`** — wraps in `{ kind: "Continue"; value: T }`. Type param affects output (loop feedback), but in practice always `any` as an escape hatch.
- **`done<T>()`** — wraps in `{ kind: "Break"; value: T }`. Type param determines loop output. Semantically justified but still annoying.

### Root cause

The branch input type is derived **bottom-up** from case handler input types. When a handler is input-agnostic (drop, recur, done), there's nothing to derive from. Invariance then rejects the incomplete type.

## Postfix `.branch()` solves this

The postfix form knows `Out` from `this`, so the branch input type comes from the preceding action — not from the case handlers:

```ts
// Out = ClassifyResult, known from classifyErrors
classifyErrors.branch({
  HasErrors: pipe(extractField<HasErrors, "errors">("errors"), forEach(fix), recur<any>()),
  Clean: done<any>(),
})
```

No type params needed on `drop` or `done` in branch cases because the branch input type isn't derived from them.

**Postfix `.branch()` should be the primary form.** The standalone `branch()` is still useful in some contexts but inherently has less type information available.

### What postfix `.branch()` should enforce

Currently the postfix `.branch()` doesn't validate `Out` against the cases at all. It should:

1. **Exhaustiveness** — require a case for every `kind` in `Out`
2. **Per-case type checking** — each handler must accept the corresponding discriminated member
3. **Contravariant handler check** — handlers are consumers, so `drop()` accepting `unknown` should pass

The postfix signature could use `Extract<Out, { kind: K }>` to derive per-case types:

```ts
branch<TCases extends { [K in KindOf<Out>]: Pipeable<Extract<Out, { kind: K }>, any, any> }>(
  cases: TCases,
): TypedAction<In, ExtractOutput<TCases[keyof TCases & string]>, Refs | ...>;
```

**Open question:** This requires `Out` to be a discriminated union with `kind`. If `Out` doesn't have `kind`, the constraint produces `never` keys and the call fails. Is that acceptable, or do we need a better error message?

**Open question:** The `Pipeable<Extract<Out, { kind: K }>, any, any>` check is invariant on the input. For `drop()` to work, we'd need contravariant-only checking here. Options:

- Use a contravariant-only phantom type for the case handler position (not full Pipeable)
- Accept `any` as the escape hatch (`drop<any>()` satisfies invariance from both sides)
- Introduce a `CaseHandler<In, Out>` type that's contravariant on In

## Standalone `branch()` — keep it verbose

The standalone `branch()` derives its input from case handler inputs. This is inherently lossy when handlers are input-agnostic. Since postfix `.branch()` is the primary form, the standalone version can stay as-is with the `BranchInput` computation requiring typed drops.

Alternatively, the standalone could take an explicit type parameter:

```ts
branch<ClassifyResult>({
  HasErrors: pipe(extractField("errors"), forEach(fix), recur()),
  Clean: done(),
})
```

This mirrors the postfix form (type comes from outside, not from handlers). But it requires a different function signature and might complicate inference.

**My opinion:** Don't bother. Push people toward postfix.

## Broader: `{ kind, value }` convention and Rust-style unions

### Current discriminated unions

Our discriminated unions have arbitrary shapes per variant:

```ts
type ClassifyResult =
  | { kind: "HasErrors"; errors: TypeError[] }
  | { kind: "Clean" };
```

Each variant has different fields — `HasErrors` has `errors`, `Clean` has nothing. The branch receives the full object and passes it to the case handler.

### Proposed: standardize on `{ kind, value }`

If all discriminated unions used `{ kind: K; value: T }`:

```ts
type ClassifyResult =
  | { kind: "HasErrors"; value: TypeError[] }
  | { kind: "Clean"; value: void };
```

Then branch could auto-extract `value` before passing to the case handler. The handler receives the unwrapped payload, not the tagged object. This is how `LoopResult` already works (`{ kind: "Continue"; value: T } | { kind: "Break"; value: T }`).

Benefits:
- Standardized structure — every union has the same shape
- Branch auto-unwraps — case handler receives payload, not tagged object
- `tag("Ok")` already produces this shape

### Proposed: phantom union membership (Rust-style enums)

In Rust, `Option::None` is distinct from any other enum's variant because variants are namespaced to their enum. In TypeScript, `{ kind: "None" }` is structurally identical regardless of which union it belongs to.

Proposed: carry the union definition as phantom data on each variant:

```ts
type TaggedUnion<TDef extends Record<string, unknown>> = {
  [K in keyof TDef & string]: { kind: K; value: TDef[K]; __def?: TDef };
}[keyof TDef & string];

// Define variant map
type ClassifyResultDef = {
  HasErrors: TypeError[];
  Clean: void;
};

type ClassifyResult = TaggedUnion<ClassifyResultDef>;
// = { kind: "HasErrors"; value: TypeError[]; __def?: ClassifyResultDef }
// | { kind: "Clean"; value: void; __def?: ClassifyResultDef }
```

The `__def` phantom carries the full variant map. Postfix `.branch()` can extract it:

```ts
type ExtractDef<T> = T extends { __def?: infer D } ? D : never;

// Postfix .branch() derives case types from __def:
branch(cases: {
  [K in keyof ExtractDef<Out> & string]: Pipeable<ExtractDef<Out>[K], any, any>
}): ...
```

This gives exhaustiveness checking (required keys come from the definition) and per-case type derivation (from the definition, not from handler inputs).

**What `__def` solves and what it doesn't:**

`__def` solves computing `BranchInput` — the branch's overall input type can be derived from the definition carried by whichever handler has a concrete type, so `drop()` doesn't need to contribute type information for that computation.

But there are two separate type checks in a branch:
1. **Pipe connection** — preceding action's output matches branch input. `__def` fixes this.
2. **Per-case handler check** — each handler must accept its variant. `__def` doesn't help here.

The per-case check still uses invariance. `drop()` with input `unknown` doesn't match `{ kind: "Clean"; value: void; __def?: ClassifyResultDef }` under invariance. You still need `drop<Clean>()` unless you ALSO relax invariance at the handler boundary to contravariant-only.

So `__def` alone doesn't eliminate type params on drop/done. You need both:
- `__def` — fixes BranchInput derivation
- Contravariant case handler checking — fixes the handler boundary

Neither is sufficient alone. Both together make the freeform branch clean.

## `tap` naming

`tap` is a bad name. It's jargon from Ruby/Rx that doesn't communicate intent. The function runs an action for side effects and preserves the original value.

Candidates:

- **`sideEffect(action)`** / **`.sideEffect(action)`** — descriptive but long
- **`aside(action)`** / **`.aside(action)`** — "do this on the side"
- **`also(action)`** / **`.also(action)`** — Kotlin uses this ("also do this")
- **`thenDo(action)`** / **`.thenDo(action)`** — "then do this (but keep my value)"
- **`perform(action)`** / **`.perform(action)`** — "perform this side effect"
- **`effect(action)`** / **`.effect(action)`** — short, clear
- **`tap(action)`** — keep it, it's an established convention in FP

**My opinion:** `also` reads well as postfix: `action.also(sideEffect)` — "do action, also do sideEffect." Short, clear, non-jargon. Kotlin precedent. `effect` is a close second.

## The invariance relaxation is the key piece

Across all the approaches discussed — postfix `.branch()`, `__def` phantom data, `{ kind, value }` convention — the thing that actually eliminates type params on `drop`/`done`/`recur` in branch cases is **contravariant case handler checking**. Everything else is about where the branch input type comes from (preceding action, `__def`, handler inputs), but the handler boundary check is what forces the explicit annotations today.

Case handlers are consumers — they receive a value and process it. A handler that accepts `unknown` can handle any variant. This is a contravariant position, and checking it invariantly is over-constrained.

Options for relaxing it:
- **Contravariant-only phantom type** for case handler positions (e.g., a `CaseHandler<In, Out>` type without the covariant `__in` field)
- **`any` escape hatch** — accept `any` as matching any type from both directions (current workaround with `drop<any>()`)
- **Postfix `.branch()` with `Extract`** — derive expected types from `Out`, check handlers against them; since `Out` provides the ground truth, the handler check can be looser

## Recommended next steps

1. **Implement postfix `.tap()` / `.also()`** — eliminates the three-type-param mess on the standalone form. Independent of everything else.
2. **Improve postfix `.branch()` type checking** — add exhaustiveness and per-case validation using `Extract<Out, { kind: K }>`. This is the path of least resistance — postfix knows `Out`, and the handler check can be designed with the right variance from the start. Independent of (1).
3. **Evaluate contravariant case handler checking** — the foundational fix that makes both postfix and freeform branches clean. Could be a `CaseHandler` type or a different encoding for branch case positions.
4. **Evaluate `{ kind, value }` convention + `__def`** — bigger change, affects handler definitions and the Rust executor. Builds on (3) for full benefit. Separate refactor doc if pursued.
