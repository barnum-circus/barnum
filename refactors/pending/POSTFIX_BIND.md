# Postfix Bind

## Motivation

`bind` and `bindInput` are standalone functions today. Every other major combinator has a postfix form on TypedAction (`.then()`, `.branch()`, `.forEach()`, `.flatten()`, `.drop()`, `.getField()`, `.pick()`, `.mapOption()`). A postfix `.bind()` and `.bindInput()` would complete the pattern:

```ts
// Today
pipe(
  listFiles(),
  bindInput<FileEntry[]>((files) =>
    files.then(forEach(processFile()))
  ),
)

// Postfix
listFiles().bindInput<FileEntry[]>((files) =>
  files.then(forEach(processFile()))
)
```

## Proposed API

### `.bindInput(body)`

```ts
// On TypedAction<In, Out, Refs>:
bindInput<TOut>(
  body: (input: VarRef<Out>) => BodyResult<TOut>,
): TypedAction<In, TOut, Refs>;
```

Captures `Out` as a VarRef. Equivalent to `this.then(bindInput(body))`.

### `.bind(bindings, body)`

```ts
// On TypedAction<In, Out, Refs>:
bind<TBindings extends Action[], TOut>(
  bindings: [...TBindings],
  body: (vars: InferVarRefs<TBindings>) => BodyResult<TOut>,
): TypedAction<In, TOut, Refs>;
```

Runs bindings concurrently (each receives `Out` as input), passes VarRefs to body. Equivalent to `this.then(bind(bindings, body))`.

## The problem: circular dependency

This is the only real obstacle. The dependency graph today:

```
ast.ts ──exports──> typedAction(), TypedAction
  ^
  │ imports
  │
bind.ts ──exports──> bind(), bindInput(), VarRef
```

`typedAction()` attaches postfix methods (`.then()`, `.branch()`, etc.) via `Object.defineProperties`. To add `.bind()` and `.bindInput()`, it would need to import from bind.ts. But bind.ts already imports from ast.ts. Circular.

### Option A: Late registration

ast.ts exposes a registration hook. bind.ts calls it at module load time.

```ts
// ast.ts
let _bindMethod: Function | null = null;
let _bindInputMethod: Function | null = null;

export function registerBindMethods(
  bindFn: Function,
  bindInputFn: Function,
) {
  _bindMethod = bindFn;
  _bindInputMethod = bindInputFn;
}

// Inside typedAction():
Object.defineProperties(action, {
  bind: { value: function(...args) {
    return this.then(_bindMethod!(...args));
  }, configurable: true },
  bindInput: { value: function(body) {
    return this.then(_bindInputMethod!(body));
  }, configurable: true },
});
```

```ts
// bind.ts (at module scope, after defining bind/bindInput)
registerBindMethods(bind, bindInput);
```

The barrel export must import bind.ts so registration runs before any user code.

**Pros:** Clean separation. No duplication of bind logic.
**Cons:** Runtime registration is a new pattern in the codebase. Methods throw if bind.ts hasn't loaded yet (shouldn't happen with correct import ordering, but it's a footgun for tests).

### Option B: Inline AST construction

The postfix methods construct the bind AST directly, without calling the standalone `bind()`. They duplicate the Handle/All/GetIndex assembly.

```ts
// Inside typedAction():
bind: { value: function(bindings, body) {
  // Duplicate the bind AST construction inline
  const effectIds = bindings.map(() => nextEffectId++);
  // ... same logic as bind() ...
  return typedAction({ kind: "Chain", first: this, rest: inner });
}, configurable: true },
```

**Pros:** No circular dependency. No registration pattern.
**Cons:** Duplicates bind's ~30 lines of AST construction. Two copies to maintain. The effectId counter would need to be shared (import from bind.ts or move to ast.ts), partially defeating the purpose.

### Option C: Don't add postfix methods

~~The postfix form is just `.then(bindInput(...))`.~~

**This doesn't work for inference.** TypeScript resolves `bindInput`'s type parameters independently before checking against `.then()`'s expected type. The explicit type annotation is still required:

```ts
// Explicit type param required — TS can't infer TIn from .then() context
listFiles().then(bindInput<FileEntry[]>((files) =>
  files.then(forEach(processFile()))
))
```

This is the same root cause as `pipe(varRef, pick("field"))` — generic arguments are resolved per-call-site, not propagated backwards from the receiving context.

So "just use `.then(bindInput(...))`" is not a substitute for a real postfix method. A postfix `.bindInput()` on TypedAction would have `Out` available directly in the method signature, avoiding the inference gap entirely.

**Pros:** Zero implementation cost.
**Cons:** Requires explicit type params, defeating the ergonomic purpose. Not a real alternative.

### Option D: Move effectId counter to ast.ts

The only shared state between bind.ts and the postfix methods is the effectId counter. Move it to ast.ts, then the postfix methods can construct the AST inline without importing bind.ts:

```ts
// ast.ts
let nextEffectId = 0;
export function allocEffectId(): number { return nextEffectId++; }
export function resetEffectIdCounter(): void { nextEffectId = 0; }
```

bind.ts imports `allocEffectId` from ast.ts. The postfix methods in `typedAction()` call `allocEffectId` directly. The readVar helper is just a pure function that builds AST — it can live anywhere.

**Pros:** No circular dependency. No registration pattern. No duplication (readVar and the Handle-building loop are small enough to factor into a shared helper in ast.ts).
**Cons:** Moves effect-system internals into ast.ts, which is currently unaware of effects. Leaks a concern.

## Recommendation

Option C is eliminated — `.then(bindInput(...))` doesn't infer, so postfix methods are genuinely needed.

Option A (late registration) is the cleanest separation. It introduces one new pattern (registration) but keeps bind logic in bind.ts where it belongs. The registration happens at module load time, so the ordering concern is theoretical — any file that uses TypedAction methods has already imported the barrel.

Option D is worth considering if we want to avoid the registration pattern. Moving the effectId counter is a small layering violation.

## Type-level considerations

The postfix `.bindInput()` needs the `BodyResult<TOut>` type (the relaxed output constraint). This is already defined in bind.ts. For option A, the postfix method delegates to the standalone function, so this works transparently. For option D, `BodyResult` would need to move to ast.ts or be duplicated.

The `InferVarRefs` mapped type would similarly need to be available wherever the postfix `.bind()` signature lives. Currently it's in bind.ts.

For the TypedAction interface definition (which lives in ast.ts), we'd need to reference `VarRef`, `InferVarRefs`, and `BodyResult`. These are all defined in bind.ts today. Adding them to the TypedAction interface creates a type-level circular reference:

```
TypedAction interface mentions VarRef
VarRef is defined as TypedAction<never, TValue>
```

This isn't a runtime circular dependency (TypeScript handles type-level cycles fine), but it means the TypedAction interface must import types from bind.ts. Since bind.ts re-exports through ast.ts, this is:

```
ast.ts (TypedAction interface) ──type import──> bind.ts
bind.ts ──type + value import──> ast.ts
```

TypeScript allows circular type imports. The value imports (typedAction, identity, drop, pipe) flow one way (bind imports from ast). So this works at the type level even if we don't restructure the runtime code.

## Summary

| Option | Circular dep? | Code duplication? | New patterns? | Inference works? |
|--------|:---:|:---:|:---:|:---:|
| A: Late registration | No | No | Yes (registration) | Yes |
| B: Inline AST | No | Yes (~30 lines) | No | Yes |
| ~~C: No postfix~~ | — | — | — | **No** (.then() can't propagate type context) |
| D: Move effectId | No | Minimal | No | Yes |
