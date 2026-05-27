# Require Explicit Type Parameters on Phantom-Typed Generics

## Motivation

`Pipeable`, `TypedAction`, `Handler`, and `CaseHandler` carry phantom type fields (`__in`, `__out`) that drive type-safe pipeline composition. All four have `= unknown` default type parameters, meaning a bare `Pipeable` silently compiles as `Pipeable<unknown, unknown>`. This defeats the purpose of the phantom fields — the type system stops tracking what flows through the pipeline.

In practice, every usage already provides explicit params except one (`Array<Pipeable>` in `recursive.ts:88`, which immediately casts). The defaults serve no purpose and create a foot-gun: if someone writes `Pipeable` without params, the compiler accepts it silently.

## Approach: Remove Defaults

Remove the `= unknown` defaults from all four types. TypeScript itself then errors on any bare usage in type position — no lint rule needed for annotations, extends clauses, or type arguments.

### Types affected

| Type | File | Line |
|------|------|------|
| `TypedAction<In = unknown, Out = unknown>` | `libs/barnum/src/ast.ts` | 200 |
| `Pipeable<In = unknown, Out = unknown>` | `libs/barnum/src/ast.ts` | 496 |
| `CaseHandler<TIn = unknown, TOut = unknown>` | `libs/barnum/src/ast.ts` | 528 |
| `Handler<TValue = unknown, TOutput = unknown>` | `libs/barnum/src/handler.ts` | 47 |

### Bare usages to fix

Only one exists today:

- `libs/barnum/src/recursive.ts:88` — `as Array<Pipeable>` becomes `as Array<Pipeable<unknown, unknown>>`

### Relationship to existing eslint rule

The existing `ESLINT_PLUGIN.md` defines `barnum/require-type-params` for *call expressions* (`loop`, `earlyReturn`, `defineRecursiveFunctions`) where inference fills in `any` silently. That rule remains useful and is orthogonal to this change — removing defaults on types handles type-position; the eslint rule handles value-position inference.

No new eslint rule is needed for this refactor.

## Open questions

1. **Should bare `Pipeable<unknown, unknown>` be allowed at all, or should there be a separate "erased" type like `AnyPipeable`?** The recursive.ts cast suggests there's at least one place that wants an existential. A type alias `type AnyPipeable = Pipeable<unknown, unknown>` would make intent explicit without needing defaults.
