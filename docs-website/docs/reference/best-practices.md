# Best Practices

## Prefer postfix methods over standalone functions

When a combinator is available as both a standalone function and a postfix method, **always prefer the postfix form.** Two reasons:

1. **No type parameters.** Standalone functions like `getField<TObj, TField>(field)` often require explicit generic arguments because TypeScript can't infer the input type without context. The postfix form `action.getField("name")` infers everything from the preceding action's output type — zero annotation needed.

2. **No wrapping in `pipe`.** Standalone functions used mid-pipeline need a `pipe(action, getField("name"))` wrapper. Postfix chains directly: `action.getField("name")`.

```ts
// Avoid: standalone requires type parameters and pipe wrapping
pipe(getUserProfile, getField<UserProfile, "email">("email"))

// Prefer: postfix infers types from context
getUserProfile.getField("email")
```

This applies to every combinator that has a postfix form: `.then()`, `.iterate()`, `.map()`, `.flatMap()`, `.filter()`, `.collect()`, `.branch()`, `.drop()`, `.tag()`, `.merge()`, `.flatten()`, `.getField()`, `.getIndex()`, `.pick()`, `.wrapInField()`, `.splitFirst()`, `.splitLast()`, `.mapErr()`, `.unwrapOr()`.

## Prefer `.then()` over `pipe()`

Postfix `.then()` is the primary way to chain steps. It reads naturally and infers types from context:

```ts
// Avoid
pipe(listFiles, Iterator.fromArray(), Iterator.map(processFile), Iterator.collect(), commit)

// Prefer
listFiles.iterate().map(processFile).collect().then(commit)
```

`pipe()` is available as an alternative but rarely needed — `.then()` chains work at any length.

## Use `taggedUnionSchema` for handler validators

When a handler returns a tagged union, use `taggedUnionSchema()`, `Option.schema()`, or `Result.schema()` instead of hand-rolling `z.discriminatedUnion()`:

```ts
// Avoid
outputValidator: z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("HasErrors"), value: z.array(errorSchema) }),
  z.object({ kind: z.literal("Clean"), value: z.null() }),
])

// Prefer
outputValidator: taggedUnionSchema({
  HasErrors: z.array(errorSchema),
  Clean: z.null(),
})
```

For `Option` and `Result` specifically:

```ts
outputValidator: Option.schema(z.string())     // Option<string>
outputValidator: Result.schema(z.string(), z.number())  // Result<string, number>
```
