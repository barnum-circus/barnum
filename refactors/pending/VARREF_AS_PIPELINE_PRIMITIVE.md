# VarRef as Pipeline Primitive

## Observation

Currently, `any` input on `TypedAction` does triple duty:
1. Sources (`constant(42)`) — produce a value, ignore input
2. VarRefs — read from state, ignore pipeline input
3. Composability — `any` means "fits anywhere in a pipe"

There's no clean type-level distinction between "I transform input" and "I produce a value from nothing."

## Key Insight: VarRef Is Not Special

A VarRef is just `TypedAction<any, T>` — a thing that produces T. It has no special semantics (no caching guarantees, no "reference" semantics distinct from any other action). It's the same primitive as `constant(42)` or `someWord` or any other action that produces a value without caring about its input.

The name "VarRef" is just what we call `TypedAction<any, T>` when it appears inside a callback. It's not a different kind of thing.

## What Happens When You Call `val.then(callback)`

### Setup

```typescript
val.then((valVarRef) => valVarRef.then(verify))
```

- `val`: `TypedAction<In, T>` — some action that produces T
- Callback: `(valVarRef: VarRef<T>) => BodyResult<U>`
- Result: `TypedAction<In, U>`

Inside the callback, `valVarRef` is conceptually the same as `val` — both are "a thing that produces T." The callback form just gives you a handle to compose with.

### Step 1: TypeScript API (compile time)

`.then(callback)` internally calls `bindInput(callback)` and chains it after `val`:

```typescript
// .then() implementation (proposed):
then(callbackOrPipeable) {
  if (typeof callbackOrPipeable === 'function') {
    return chain(this, bindInput(callbackOrPipeable));
  }
  return chain(this, callbackOrPipeable);
}
```

### Step 2: bindInput expansion

`bindInput(callback)` = `bind([identity()], ([input]) => pipe(drop, callback(input)))`

At build time:
1. Allocate a ResumeHandlerId `r0`
2. Create a VarRef (ResumePerform node pointing to `r0`)
3. Call `callback(varRef)` → returns the user's pipeline AST (e.g., `Chain(ResumePerform(r0), verify)`)
4. Wrap in: `pipe(drop, userPipeline)` → `Chain(Drop, Chain(ResumePerform(r0), verify))`

### Step 3: AST produced

```
Chain(
  val,                                    // run val, produce T
  Chain(
    All(Identity, Identity),              // duplicate: [T, T]
    ResumeHandle(r0, readVar(0),          // install handler: r0 reads slot 0
      Chain(
        GetIndex(1).Unwrap,               // extract pipeline_input (= T)
        Chain(
          Drop,                           // discard it (→ null)
          Chain(
            ResumePerform(r0),            // fire handler → reads T back from state
            verify                        // apply verify to T
          )
        )
      )
    )
  )
)
```

### Step 4: Rust engine execution

1. Run `val` → produces value `T`
2. `All(Identity, Identity)` → `[T, T]` (tuple)
3. ResumeHandle installs handler for `r0`: when fired, reads index 0 of tuple → `T`
4. `GetIndex(1).Unwrap` → extracts `T` (the pipeline_input copy)
5. `Drop` → discards it, pipeline value is now `null`
6. `ResumePerform(r0)` → fires handler → handler returns `T` from state
7. `verify` → receives `T`, produces `U`

**Net effect:** `T` goes into state and comes right back out. Round-trip for zero benefit in this case.

### Compare: `val.then(verify)` (direct Pipeable form)

```
Chain(val, verify)
```

Execution: Run `val` → `T` → `verify` → `U`. Two nodes. No state. No round-trip.

### When the callback form IS worth it

```typescript
val.then((v) => allObject({
  original: v,
  transformed: v.then(verify),
}))
```

The VarRef `v` is used in two places. The `bindInput` machinery makes this expressible. For single-use, the direct Pipeable form produces a simpler AST.

## Pipeline Positions

Every action occupies a position in a pipeline. Today they're all `TypedAction<In, Out>` with `any`/`void`/specific-type doing the disambiguation. What if the positions were distinct types?

### The Four Positions

| Position   | Type                     | Meaning                                        |
| ---------- | ------------------------ | ---------------------------------------------- |
| Start      | `Pipeable<void, T>`      | Produces T from nothing. Entry point.          |
| Middle     | `Pipeable<In, Out>`      | Transforms In → Out. Composable.               |
| End        | `Pipeable<In, void>`     | Consumes In, produces nothing. Side-effect.    |
| Standalone | `Pipeable<any, unknown>` | Fully erased. No type constraints. Executable. |

- **Start** = `constant(42)`, source handlers — typed output, no input needed
- **Middle** = `verify`, `getField("name")` — typed input and output
- **End** = side-effect-only (typed input, void output)
- **Standalone** = type-erased action. The thing you hand to the runtime. `any` input, `unknown` output. What you get after composition is done and types don't matter anymore.

### Collapse

End is just a Middle where Out = void. Not structurally distinct.

Start vs Middle: the real question. Is `void` input vs specific input worth encoding as separate types, or is it just a type parameter value?

Standalone is the fully-erased runtime form. It's what `runPipeline` (no input) must accept.

## Composition Rules

```
Source<A>.then(Transform<A, B>)      → Source<B>
Transform<A, B>.then(Transform<B, C>) → Transform<A, C>
Source<A>                              → runnable (produces A)
Transform<A, B>                        → not runnable (needs A)
```

## Where Positions Appear Today

| Context                        | What's accepted                | Position semantics         |
| ------------------------------ | ------------------------------ | -------------------------- |
| `config(___)`                  | `TypedAction<void, T>`         | Start                      |
| `pipe(first, ...rest)`         | first: any, rest: Transforms   | Start + Middles            |
| `.then(___)`                   | `Pipeable<Out, TNext>`         | Middle                     |
| `allObject({ a: ___, b: ___})` | `Pipeable<In, T>`             | Middles (shared input)     |
| `bindInput(callback)`          | callback returns BodyResult    | Callback produces a Source |

## Could This Be Just a Type Parameter?

Maybe the distinction is a phantom type parameter on the action:

```typescript
type Position = "Source" | "Transform";

type Action<TPos extends Position, TIn, TOut> = ...

type Source<T> = Action<"Source", void, T>;
type Transform<In, Out> = Action<"Transform", In, Out>;
```

Or even simpler — it's already encoded in the input type:
- `In = void` → Source (the `PipeIn` helper already erases void → any for pipe compatibility)
- `In = specific` → Transform

The question is whether making this distinction EXPLICIT (via a type parameter or separate types) buys us something over the current implicit encoding.

## What a Clean Two-Type World Looks Like

### Type Definitions

```typescript
// A value that will be produced. No input.
type Source<T> = {
  __out?: () => T;
  then<U>(step: Transform<T, U>): Source<U>;
};

// A function that transforms input to output.
type Transform<In, Out> = {
  __in?: (input: In) => void;
  __out?: () => Out;
  then<U>(step: Transform<Out, U>): Transform<In, U>;
};
```

### Composition

```typescript
// Source chains with Transform to produce Source
Source<A>.then(Transform<A, B>) → Source<B>

// Transform chains with Transform to produce Transform
Transform<A, B>.then(Transform<B, C>) → Transform<A, C>

// VarRef IS a Source (just a name, not a distinct type)
VarRef<T> = Source<T>

// bindInput: callback that receives a Source → wraps into Transform
bindInput<TIn, TOut>(
  body: (input: Source<TIn>) => Source<TOut>
): Transform<TIn, TOut>
```

### How allObject Works

```typescript
// All Transforms with same input:
allObject({ a: Transform<In, A>, b: Transform<In, B> }) → Transform<In, {a: A, b: B}>

// All Sources (no shared input needed):
allObject({ a: Source<A>, b: Source<B> }) → Source<{a: A, b: B}>

// Mixed? A Source IS independent of input, so:
allObject({ a: Transform<In, A>, b: Source<B> }) → Transform<In, {a: A, b: B}>
// Source<B> can appear alongside Transforms because it doesn't constrain In.
```

## The Trade-off

**Benefits of explicit Source/Transform:**
- Pipeline position is visible in the type. You can't accidentally pass a Transform where a Source is needed.
- `run()` only accepts Source — statically prevents "trying to run an incomplete pipeline."
- `allObject` can cleanly handle mixed Sources and Transforms.
- The `any` hack disappears. No more "input any means it fits anywhere."

**Costs:**
- Two types instead of one. More cognitive overhead.
- Every combinator needs overloads or conditional types to handle both.
- Source<T> "promoting" to work alongside Transforms (in allObject) needs explicit rules.
- Is this just `void` vs non-void input with extra steps?

## Key Question

Is this actually two distinct types, or is it just a type parameter (`void` vs `TIn`) that's already implicit in today's system? The current `PipeIn<T>` helper already treats void specially. Maybe the "clean abstraction" is just making `void`-input the official marker for Sources and stopping the use of `any`-input entirely.

In that world:
- `constant(42): TypedAction<void, number>` (not `any`)
- VarRef<T> = `TypedAction<void, T>` — same as any other source. No special semantics.

The `void` input means "I don't depend on pipeline input." Whether the engine re-executes or caches is an implementation detail, not a type-level concern.

## Future Direction: Multi-Input Pipelines (Orthogonal)

If `bindInput<[A, B]>((a, b) => ...)` destructures a tuple into separate VarRefs, then conceptually the callback is a pipeline with MULTIPLE inputs. This reframes the model:

- Source = zero inputs (`() → T`)
- Transform = one input (`A → B`)
- Multi-input = N inputs (`(A, B, ...) → Out`)

The distinction between Source/Transform isn't binary — it's arity. `.then()` connects one output to one input port. Multi-input pipelines need the other ports filled from elsewhere (VarRefs, bind).

This is a separate exploration from the current VarRef/callback question.
