# Standalone Barnum Syntax

## Motivation

Barnum is currently an embedded DSL inside TypeScript. The TS combinators (`pipe`, `branch`, `tryCatch`, etc.) construct an AST that gets flattened and shipped to a Rust engine. This works, but TypeScript carries baggage:

1. **The type encoding is a hack.** Phantom fields, invariance via paired covariant/contravariant fields, `CaseHandler` vs `Pipeable` vs `TypedAction` — all of this exists to trick TypeScript's type system into enforcing Barnum's actual type rules. A native type system would enforce them directly.
2. **HOAS requires callbacks.** `tryCatch((throwError) => ...)`, `loop((recur, done) => ...)` — the callback is a construction-time closure that produces AST. The callback itself is a TypeScript artifact. In a native language, scoped bindings are just scoped bindings.
3. **Handlers are defined in TS, referenced by module path.** The handler's type signature exists twice: once in the Zod/TS validator, once as phantom types on the `TypedAction`. A native language would declare the type once.
4. **The TS runtime is dead weight for control flow.** The TS DSL exists only to produce JSON. A `.barn` file compiled directly to the flat config by a Rust compiler eliminates Node.js from the build pipeline entirely.

The goal: a `.barn` file format that compiles to the same flat config JSON the TS DSL produces. The engine and runtime are unchanged. The standalone syntax is an alternative frontend.

## Syntax sketch

### Types

```barnum
type FileEntry = {
  path: String,
  content: String,
}

type BuildResult = {
  success: Bool,
  output: String,
}

// Tagged unions (sum types) — the { kind, value } convention from the TS DSL
// is the compilation target, not the surface syntax. In .barn files, variants
// are declared with standard algebraic data type syntax.
//
// Compilation target uses namespaced kind strings: { kind: "Shape.Circle", value: ... }
// The enum name is the namespace prefix, dot-separated from the variant name.
type Shape =
  | Circle(Float)
  | Rect({ width: Float, height: Float })
  | Point                                   // unit variant (value = void)

// Built-ins, always in scope:
//   Option<T>      = Some(T) | None
//   Result<T, E>   = Ok(T) | Err(E)
//   Iterator<T>    = Iterator(T[])          // eager, backed by array
//   LoopResult<C, B> = Continue(C) | Break(B)
```

Types are structural for objects and nominal for tagged unions. An object `{ path: String, content: String }` matches any handler expecting those fields. A tagged union `Shape` is distinct from another union with the same variants — the union name matters for exhaustiveness checking.

The compilation target for tagged unions uses **namespaced kind strings**: `Shape.Circle`, not `Circle`. The enum name is the namespace prefix. This enables two-level dispatch (see "branchFamily" in the desugaring section) — the engine can route first on the enum prefix, then on the variant, which is how polymorphic methods like `.map()` and `.unwrapOr()` dispatch across Option/Result/Iterator without runtime metadata.

### Handler declarations

Handlers are foreign functions. They're implemented in TypeScript, Rust, Python, or any language that speaks the handler protocol. The `.barn` file declares their type signature and where to find the implementation.

```barnum
// Full form
handler listFiles: Void -> FileEntry[]
  from "./handlers/files.ts" as "listFiles"

// Short form — function name defaults to the handler name
handler listFiles: Void -> FileEntry[]
  from "./handlers/files.ts"

// Batch import
from "./handlers/steps.ts" import {
  stepA: Void -> Result<Void, String>,
  stepB: Void -> Result<Void, String>,
  stepC: Void -> Result<Void, String>,
  logError: String -> Void,
}
```

Handler declarations are the FFI boundary. The compiler trusts the declared type signature — it doesn't parse the TypeScript source to verify it. (Verification is a separate concern; see "Handler type verification" below.)

### Pipelines

The pipe operator `|>` is the primary sequencing mechanism. It desugars to Chain.

```barnum
// Three-stage pipeline
listFiles |> forEach(buildFile) |> logResults

// Equivalent to the TS DSL:
//   pipe(listFiles, forEach(buildFile), logResults)
```

The pipeline is the basic unit of composition. Every workflow is a pipeline.

### Builtins

Builtins are the ALU operations that run inline in Rust (no subprocess). The full list of builtins in the engine:

| Builtin | Description | TS DSL equivalent |
|---|---|---|
| **Constant** | Return a fixed value, ignoring input | `constant(value)` |
| **Identity** | Return input unchanged | `identity()` |
| **Drop** | Discard input, return null | `drop()` |
| **GetField** | Extract a named field from an object | `.getField("name")` |
| **GetIndex** | Extract element by index, returns **Option\<T\>** | `.getIndex(n)` |
| **Pick** | Select named fields from object | `.pick("a", "b")` (composed) |
| **WrapInField** | Wrap input as `{ field: input }` | `.wrapInField("field")` |
| **Merge** | Merge array of objects into single object | `merge()` |
| **Flatten** | Flatten nested array one level | `.flatten()` |
| **SplitFirst** | Head/tail decomposition → Option\<[T, T[]]\> | `.splitFirst()` |
| **SplitLast** | Init/last decomposition → Option\<[T[], T]\> | `.splitLast()` |
| **Slice** | Slice array from start (inclusive) to end (exclusive) | `slice(start, end?)` |
| **CollectSome** | Collect Some values from Option\<T\>[], discard Nones | `Option.collect()` |
| **ExtractPrefix** | Extract enum namespace from kind field (two-level dispatch) | `extractPrefix()` |
| **AsOption** | Convert boolean to Option\<void\> | `.asOption()` |
| **Sleep** | Async sleep for fixed ms, then return null | `sleep(ms)` |
| **Panic** | Halt execution with fatal error (not caught by tryCatch) | `panic("msg")` |

Note: **Pick** is currently a composed operation (GetField + WrapInField + All + Merge) rather than a dedicated builtin variant. **GetIndex** returns `Option<T>` — `Some(element)` for valid indices, `None` for out-of-bounds. This is a departure from typical array indexing and affects the surface syntax.

In the native syntax, most builtins have dedicated syntax rather than function calls:

```barnum
// Field access — desugars to GetField
value |> .name              // extract "name" from the pipeline value

// Indexing — desugars to GetIndex (returns Option<T>!)
value |> .[0]               // extract index 0 → Option<T>
value |> .[0]!              // extract index 0, unwrap (panics on out-of-bounds)

// Pick — desugars to composed GetField+WrapInField+All+Merge
value |> .{name, age}       // select fields "name" and "age"

// WrapInField — wrap input value inside a field
value |> .name = _          // { name: value } — syntax TBD

// Tag — desugars to Constant(namespaced kind) + WrapInField + Merge
// The compiler resolves @Ok to the full namespaced kind "Result.Ok" from context.
value |> @Ok                // wrap as { kind: "Result.Ok", value: value }

// Drop — desugars to Drop
value |> _                  // discard the value

// Constant — desugars to Constant
_ |> 42                     // ignore input, produce 42
_ |> "hello"                // ignore input, produce "hello"

// Identity — no syntax needed, implicit in pipelines

// Merge — desugars to Merge (on tuple output from all())
all(a, b) |> merge          // merge tuple of objects into one

// Flatten — desugars to Flatten
nested_lists |> flatten     // flatten nested arrays

// SplitFirst / SplitLast — head/tail and init/last decomposition
items |> splitFirst         // Option<[first, rest]>
items |> splitLast          // Option<[init, last]>

// Slice / Take / Skip — array and iterator slicing
items |> slice(2, 5)        // elements at indices [2, 5)
items |> take(3)            // first 3 elements (sugar for slice(0, 3))
items |> skip(2)            // drop first 2 (sugar for slice(2))

// Panic — fatal, uncatchable error
value |> panic("invalid state")
```

The `.field`, `.[index]`, `.{fields}` syntax replaces the TS DSL's `.getField("field")`, `.getIndex(n)`, `.pick("a", "b")`. The `@Variant` syntax replaces `.tag("Variant", "EnumName")` — the compiler resolves the enum prefix from the expected type context. The `_` discard replaces `.drop()`.

### Pattern matching (Branch)

```barnum
// On a tagged union — desugars to Branch with auto-unwrap
fetchResult |> match {
  Ok(value) => processValue,
  Err(error) => handleError,
}

// On a LoopResult
pollStatus |> match {
  Continue(input) => recur,
  Break(result) => done,
}
```

`match` is exhaustive — omitting a variant is a compile error. Each arm receives the unwrapped payload (the `value` field of the `{ kind, value }` pair). This replaces the TS DSL's `.branch({ Ok: ..., Err: ... })`.

### Concurrent execution (All, ForEach)

```barnum
// all() — run actions concurrently, collect results as a tuple
all(fetchFromApi, readFromDb, checkCache)

// forEach — apply action to each element of an array
forEach(processItem)

// Combining: fan-out then merge
all(computeA, computeB) |> merge
```

### Bind (let-bindings)

`let` introduces concurrent bindings that are available as named references throughout the body. Desugars to the same ResumeHandle/ResumePerform/All structure as the TS DSL's `bind`.

```barnum
let files = listFiles,
    config = loadConfig
in {
  // files and config are in scope here — they're VarRefs
  files |> forEach(processFile(config))
}

// let-input — capture the pipeline input as a named reference
let input = _
in {
  input |> .name |> validate |> combine(input |> .age)
}
```

`let` bindings evaluate concurrently (all bindings run in parallel) and are available throughout the body. This replaces the TS DSL's `bind([...], ([x, y]) => ...)`.

### Error handling (tryCatch)

```barnum
try (throw) {
  stepA |> unwrapOr(throw) |> _
  |> stepB |> unwrapOr(throw) |> _
  |> stepC
} catch (error) {
  logError
}
```

The `try` block introduces `throw` — a scoped effect token typed `TError -> Never`. When `throw` fires, the body is torn down and `catch` runs with the error. This replaces the TS DSL's `tryCatch((throwError) => ..., recovery)`.

`throw` is not a keyword — it's a binding introduced by the `try` block. You could name it anything:

```barnum
try (fail) {
  stepA |> unwrapOr(fail)
} catch (error) {
  recovery
}
```

### Loop

```barnum
loop (recur, done) {
  body |> match {
    Continue => recur,
    Break => done,
  }
}
```

`recur` and `done` are scoped effect tokens. `recur` restarts the loop body with a new input. `done` exits the loop with the break value. Desugars to RestartHandle/RestartPerform/Branch, same as the TS DSL.

### Recur (simple restart)

```barnum
recur (restart) {
  body |> restart    // restart the body with a new input
}
```

`recur` is a simpler form of `loop` — it provides a single `restart` token with no `done`. The body either completes normally (the value exits) or restarts. Useful when the loop exit condition is handled elsewhere (e.g., by a match arm that doesn't call restart). Desugars to a single RestartHandle/RestartPerform without the Continue/Break branching wrapper that `loop` adds.

### Early return

```barnum
earlyReturn (exit) {
  // exit : T -> Never — exits the scope immediately
  condition |> match {
    Invalid => exit,
    Valid => proceed,
  }
}
```

### Race

```barnum
race(
  fetchFromPrimary,
  fetchFromSecondary,
  fetchFromCache,
)
```

First action to complete wins. Losers are cancelled. All branches must have the same type.

### Timeout

```barnum
withTimeout(2000, longRunningHandler)
// Returns: Result<T, Void> — Ok if completed, Err if timed out
```

In the TS DSL, `withTimeout(ms, body)` takes `ms` as a `Pipeable<TIn, number>` — the timeout duration is itself an action, not a raw number. In `.barn` syntax, a literal integer is automatically wrapped in a Constant. If the timeout needs to be computed from the pipeline input, pass an expression: `withTimeout(.timeoutMs, body)`.

### Steps (named actions / function calls)

```barnum
step Validate(input): ValidationResult =
  validate |> match {
    Valid => processValid,
    Invalid => step Fix,
  }

step Fix(error): ValidationResult =
  autoFix |> step Validate

workflow main: Void -> ValidationResult =
  prepareInput |> step Validate
```

Steps are named actions that can reference each other (mutual recursion). `step Foo` is a jump to Foo's body — mechanically a function call, with Chain providing the return address.

In the TS DSL, this is implemented by `defineRecursiveFunctions`, which uses ResumeHandle/ResumePerform for variable capture (each function is a VarRef) and mutual references are captured at construction time.

### Iterator

Iterators are an abstraction over arrays wrapped in a tagged union: `{ kind: "Iterator.Iterator", value: T[] }`. They are **eager** (backed by a real array), not lazy — no short-circuiting, no infinite iterators.

```barnum
// Enter Iterator from various types
items |> iterate            // dispatches: Array → fromArray, Option → fromOption, Result → fromResult

// Transform elements
items |> iterate |> map(transform) |> collect

// Flat-map (action returns any IntoIterator: Iterator, Option, Result, or Array)
items |> iterate |> flatMap(action) |> collect

// Filter
items |> iterate |> filter(predicate) |> collect

// Fold
items |> iterate |> fold(init, body)

// Decomposition
items |> iterate |> splitFirst    // Option<[T, Iterator<T>]>
items |> iterate |> splitLast     // Option<[Iterator<T>, T]>

// Slicing
items |> iterate |> take(3)       // first 3 elements
items |> iterate |> skip(2)       // drop first 2
items |> iterate |> slice(2, 5)   // elements at indices [2, 5)

// Check emptiness
items |> iterate |> isEmpty       // boolean

// Collect back to array
items |> iterate |> collect
```

Iterator methods operate on the `Iterator<T>` tagged union. `.iterate()` is the entry point — it uses `branchFamily` (ExtractPrefix + Branch) to dispatch based on the input type's enum prefix. `.collect()` exits the Iterator back to `T[]`.

Note: `collect` also dispatches via `branchFamily` — on `Option<T>[]` (an array of Options), it routes to CollectSome (discards Nones, unwraps Somes). On `Iterator<T>`, it routes to `getField("value")`.

### Full example: retry-on-error demo

The TS DSL version:

```typescript
earlyReturn<any, never, any>((earlyReturn) =>
  loop<any, any>((recur, done) =>
    pipe(
      drop<any>(),
      tryCatch(
        (throwError) =>
          pipe(
            stepA.unwrapOr(throwError).drop(),
            withTimeout(constant(2_000), stepB.unwrapOr(throwError))
              .mapErr(constant("stepB: timed out"))
              .unwrapOr(throwError)
              .drop(),
            stepC.mapErr(drop()).unwrapOr(earlyReturn),
            done,
          ),
        logError.drop().then(recur),
      ),
    ),
  ),
)
```

The `.barn` version:

```barnum
from "./handlers/steps.ts" import {
  stepA: Void -> Result<Void, String>,
  stepB: Void -> Result<Void, String>,
  stepC: Void -> Result<Void, String>,
  logError: String -> Void,
}

workflow main: Void -> Void =
  earlyReturn (exit) {
    loop (recur, done) {
      _
      |> try (throw) {
        stepA |> unwrapOr(throw) |> _
        |> withTimeout(2000, stepB |> unwrapOr(throw))
           |> mapErr(_ |> "stepB: timed out")
           |> unwrapOr(throw)
           |> _
        |> stepC |> mapErr(_) |> unwrapOr(exit)
        |> done
      } catch {
        logError |> _ |> recur
      }
    }
  }
```

### Full example: polling loop

```barnum
handler startPolling: Void -> PollInput
  from "./handlers/start-polling.ts"

handler pollStatus: PollInput -> LoopResult<PollInput, PollResult>
  from "./handlers/poll-status.ts"

workflow main: Void -> PollResult =
  startPolling |> loop (recur, done) {
    pollStatus |> match {
      Continue => recur,
      Break => done,
    }
  }
```

## Compilation pipeline

```
.barn source
    |
    v
  Lexer/Parser (Rust)
    |
    v
  Tree AST (same Action enum, or a richer HIR)
    |
    v
  Type Checker
    |
    v
  Desugar (lower syntactic sugar to core AST)
    |
    v
  Flatten (same flatten() pass, producing FlatConfig)
    |
    v
  .barnum.json (the flat config — identical format to TS DSL output)
    |
    v
  Engine (unchanged)
```

The compiler is written in Rust and lives in the same repo as the engine. Its output is the same flat config JSON that the TS DSL produces. The engine doesn't know or care which frontend produced the config.

### Core AST (compilation target)

The engine operates on 9 action variants:

| Action | Description |
|---|---|
| **Invoke** | Leaf node — invokes an external handler (TypeScript) or builtin |
| **Chain** | Binary sequential composition: run `first`, feed output to `rest` |
| **ForEach** | Parallel map over array input |
| **All** | Fanout: same input to all actions, collect results as tuple |
| **Branch** | N-ary branch on `kind` field of discriminated union |
| **ResumeHandle** | Resume-style effect handler — handler runs inline, produces `[value, new_state]` |
| **ResumePerform** | Raise resume-style effect (targets enclosing ResumeHandle) |
| **RestartHandle** | Restart-style effect handler — body torn down, handler output re-advances body |
| **RestartPerform** | Raise restart-style effect (targets enclosing RestartHandle) |

The two effect flavors:
- **Resume**: handler runs inline at the Perform site, returns a value to the Perform's parent, and writes new state back to the handle frame. Used for `let`/`bind` (VarRefs), `defineRecursiveFunctions` (function refs).
- **Restart**: body is torn down when the effect fires, handler's output becomes the new body input, body re-executes from scratch. Used for `tryCatch`, `loop`, `earlyReturn`, `race`, `recur`.

### Desugaring

Most surface syntax is sugar over these 9 actions:

| Surface syntax | Desugars to |
|---|---|
| `try (t) { body } catch { recovery }` | `RestartHandle(id, Chain(Tag("LoopResult.Continue"), body_wrapped_in_branch), handler)` |
| `loop (r, d) { body }` | `Chain(Tag("LoopResult.Continue"), RestartHandle(id, Branch({ Continue: body, Break: Identity }), handler))` |
| `recur (r) { body }` | `RestartHandle(id, body, GetIndex(0))` — simpler, no Continue/Break wrapper |
| `earlyReturn (e) { body }` | Same restart substrate as loop — Break path exits, Continue path runs body |
| `race(a, b, c)` | `RestartHandle(id, All(a_tagged_break, b_tagged_break, ...), handler)` |
| `let x = a, y = b in { body }` | `Chain(All(a, b, Identity), ResumeHandle(e0, ResumeHandle(e1, ..., body), handler))` |
| `step` (mutual recursion) | `ResumeHandle` per function ref, body uses `ResumePerform` to call |
| `match { A => x, B => y }` | `Branch({ "A": Chain(GetField("value"), x), "B": Chain(GetField("value"), y) })` |
| `.field` | `Invoke(Builtin(GetField("field")))` |
| `@Variant` (in Result context) | `Chain(All(Constant("Result.Variant"), WrapInField("kind")), WrapInField("value")), Merge)` |
| `_` | `Invoke(Builtin(Drop))` |
| `42` (literal) | `Invoke(Builtin(Constant(42)))` |
| `iterate` | `Chain(ExtractPrefix, Branch({ Option: fromOption, Result: fromResult, Array: fromArray }))` |
| `collect` | `Chain(ExtractPrefix, Branch({ Array: CollectSome, Iterator: GetField("value") }))` |
| `slice(2, 5)` | `Invoke(Builtin(Slice { start: 2, end: Some(5) }))` |
| `take(3)` | `Invoke(Builtin(Slice { start: 0, end: Some(3) }))` |
| `skip(2)` | `Invoke(Builtin(Slice { start: 2, end: None }))` |

The desugaring is mechanical and produces the same AST the TS DSL would produce. The compiler and engine share the `barnum_ast` crate.

**branchFamily**: Many postfix methods (`.map()`, `.unwrapOr()`, `.filter()`, `.iterate()`, `.collect()`) work polymorphically across Option, Result, and Iterator. This is implemented as two-level dispatch: `Chain(ExtractPrefix, Branch({ Result: ..., Option: ..., Iterator: ... }))`. The ExtractPrefix builtin extracts the enum namespace from the `kind` field (e.g., `"Result.Ok"` → `{ kind: "Result", value: original }`), then Branch dispatches on the namespace. In `.barn` syntax, this dispatch is implicit — the compiler generates it from the type context.

## Type system

### Design principles

1. **Forward-flowing inference.** Types propagate left-to-right through pipelines. The output type of each stage determines the input type of the next. No backwards inference needed for the common case.
2. **Explicit handler signatures.** Handler types are declared at the FFI boundary. No inference across the handler boundary.
3. **Structural objects, nominal unions.** Object types use structural compatibility (like TS). Tagged unions are nominal — `Shape` is distinct from another union with the same variants.
4. **Invariant pipeline connections.** Every pipeline connection point is invariant: the output type of stage N must exactly match the input type of stage N+1. No covariance, no contravariance at pipeline joints. (The TS DSL achieves this with paired phantom fields; the native type system enforces it directly.)
5. **Effect scope checking.** Effect tokens (throw, recur, done, exit) are typed and scoped. Using a token outside its scope is a compile error. This is static — no runtime checking needed.

### Type grammar

```
Type = String | Number | Bool | Void | Never
     | { field: Type, ... }              -- object
     | Type[]                             -- array
     | Name                               -- named type reference
     | Name<Type, ...>                    -- generic application
     | A | B | C                          -- union (for tagged unions only)
     | Type -> Type                       -- action/handler type
```

### What the type checker validates

1. **Pipeline compatibility**: in `a |> b`, the output type of `a` must equal the input type of `b`.
2. **Match exhaustiveness**: every variant of the input union must have a corresponding arm.
3. **Effect scoping**: `throw`, `recur`, `done`, etc. can only appear within their enclosing `try`/`loop`/`earlyReturn` block. The type `T -> Never` on these tokens prevents using their output in a pipeline (there is no output).
4. **Step reference resolution**: `step Foo` must resolve to a declared step. Mutual recursion is allowed.
5. **Generic instantiation**: `forEach(handler)` checks that `handler: T -> U` and infers `T[] -> U[]`.
6. **Handler arity**: a handler declared `A -> B` accepts exactly `A` as input and produces exactly `B`.

### What the type checker does NOT validate

1. **Handler implementation correctness.** The compiler trusts handler type declarations. If a TypeScript handler is declared `String -> Number` but actually returns a string, the Rust engine will see the wrong type at runtime. (This is what the Validate builtin from HANDLER_VALIDATION.md addresses.)
2. **Handler purity/determinism.** The compiler doesn't know what handlers do internally.
3. **Termination.** Loops may not terminate. Steps can create infinite mutual recursion. The compiler doesn't prove termination.

### Implementation approach

A **bidirectional type checker** (like Koka, Elm, and PureScript use). The algorithm:

- **Check mode**: when the expected type is known (e.g., the right side of `|>` where the left side's output type constrains the expected input), propagate the type inward.
- **Infer mode**: when no expected type exists (e.g., the first action in a pipeline), infer the type outward.

For Barnum, most type checking is forward inference through pipelines. The main place where check mode matters is in handler arguments and match arms, where the expected type comes from the union definition.

## Build process and tooling

### CLI commands

```bash
barnum check workflow.barn           # type check only
barnum build workflow.barn           # compile to .barnum.json
barnum run workflow.barn             # compile + run
barnum run workflow.barnum.json      # run pre-compiled
barnum fmt workflow.barn             # format source
barnum lsp                           # start language server
barnum init                          # create a new project
barnum codegen workflow.barn         # generate handler stubs (see below)
```

### Project structure

```
my-workflow/
  barnum.toml              # project config (handler search paths, target, etc.)
  src/
    main.barn              # workflow definition
    types.barn             # shared type definitions (imported by main.barn)
  handlers/
    steps.ts               # TypeScript handler implementations
    build.rs               # Rust handler implementations (future)
  out/
    main.barnum.json       # compiled flat config
```

`barnum.toml` replaces the implicit configuration that the TS DSL gets from `package.json` and `tsconfig.json`:

```toml
[project]
name = "my-workflow"

[build]
entry = "src/main.barn"
output = "out/main.barnum.json"

[handlers]
search_paths = ["handlers/"]
default_lang = "typescript"
```

### Handler type verification

The compiler trusts handler type declarations by default. For stronger guarantees, two approaches:

**A. Schema extraction (build-time).** `barnum check --verify-handlers` parses handler source files and extracts type information. For TypeScript handlers with Zod validators, it reads the Zod schema and compares it against the declared `.barn` type. For TypeScript handlers with JSDoc or explicit type annotations, it reads those. This is best-effort — it works for well-typed handlers and skips handlers it can't analyze.

**B. JSON Schema embedding (compile-time).** The compiler converts `.barn` type declarations to JSON Schema and embeds them in the flat config as Validate builtins at handler call sites. The engine validates handler output at runtime. This is the approach from HANDLER_VALIDATION.md — the `.barn` compiler just generates the same Validate nodes automatically, without the user writing `.validated()` or `.strict()`.

These approaches complement each other: A catches errors early (build time), B catches errors that slip through (runtime).

### Handler stub generation

`barnum codegen workflow.barn` reads the handler declarations in the `.barn` file and generates implementation stubs in the target language:

```bash
barnum codegen src/main.barn --lang typescript --out handlers/
```

Produces:

```typescript
// handlers/steps.ts (generated)
import { createHandler } from "@barnum/barnum";
import { z } from "zod";

export const stepA = createHandler({
  inputValidator: z.void(),
  outputValidator: z.discriminatedUnion("kind", [
    z.object({ kind: z.literal("Result.Ok"), value: z.void() }),
    z.object({ kind: z.literal("Result.Err"), value: z.string() }),
  ]),
  handle: async ({ value }) => {
    // TODO: implement
    throw new Error("Not implemented");
  },
});
```

The `.barn` file is the source of truth for handler types. Code generation flows outward from it.

## IDE support (LSP)

### Language server

`barnum lsp` starts a Language Server Protocol server. The server provides:

| Feature | What it does |
|---|---|
| Diagnostics | Type errors, unresolved references, exhaustiveness violations |
| Hover | Show inferred type at any pipeline point |
| Go to definition | Jump to handler declaration, type definition, step definition |
| Find references | All uses of a handler, type, step, or effect token |
| Completion | Handler names, field names (from known object types), match arms |
| Rename | Rename handlers, types, steps across the project |
| Signature help | Show handler type when typing a handler call |
| Inlay hints | Inferred types at pipeline connection points |

### Syntax highlighting

A Tree-sitter grammar for `.barn` files. Tree-sitter grammars are consumed by VS Code (via extensions), Neovim, Helix, Zed, and most modern editors. One grammar covers all editors.

### VS Code extension

A minimal extension that:
1. Registers `.barn` file association
2. Bundles the Tree-sitter grammar for syntax highlighting
3. Starts the LSP server for intelligence features

## Migration from the TS DSL

Both frontends produce identical flat config JSON. They coexist indefinitely — there's no forced migration.

The TS DSL remains the right choice when:
- The workflow is defined programmatically (generated from config, computed at startup)
- The project already has complex TypeScript handler infrastructure
- The team prefers staying in one language

The `.barn` syntax is better when:
- The workflow is static (doesn't change at runtime)
- The workflow is the artifact you want to read, review, and version
- Handler implementations span multiple languages (TS, Rust, Python)
- You want the compiler to own type checking instead of hacking TypeScript's type system

### Incremental adoption

A single project can mix both. The compiled flat config is the universal interchange:

```
workflow-a.barn  --barnum build-->  workflow-a.barnum.json  --barnum run-->  engine
workflow-b.ts    --tsx-->           workflow-b.barnum.json   --barnum run-->  engine
```

Handlers are reusable across both frontends — they're just TypeScript modules referenced by path.

## Implementation steps

### Step 1: Grammar and parser

Define the grammar formally (EBNF or PEG). Implement the lexer and parser in Rust, producing a tree AST. The AST can be a superset of the existing `Action` enum (richer node types for surface syntax like `try`/`catch`, `let`/`in`, `match`), which gets desugared to the core `Action` enum before flattening.

Crate: `barnum_syntax` (new). Dependencies: none (hand-written recursive descent, or a parser generator like `lalrpop` / `chumsky`).

### Step 2: Type checker

Implement a bidirectional type checker operating on the tree AST. The type checker:
- Resolves type names to their definitions
- Checks pipeline compatibility (output matches next input)
- Checks match exhaustiveness
- Checks effect token scoping
- Reports errors with source locations

Crate: `barnum_typecheck` (new). Dependencies: `barnum_syntax`.

This is the hardest step. The type system is simple compared to TypeScript or Rust (no traits, no lifetimes, no complex inference), but it's still a type checker from scratch. The closest comparison in complexity is Elm's type checker — structural records, algebraic data types, simple generics, forward inference.

### Step 3: Desugaring

Lower the surface AST to the core `Action` enum (9 variants: Invoke, Chain, ForEach, All, Branch, ResumeHandle, ResumePerform, RestartHandle, RestartPerform). This is mechanical: each surface construct (`try`/`catch`, `loop`, `recur`, `let`/`in`, `iterate`, `collect`, etc.) maps to a fixed pattern of these actions. The TS DSL's combinator functions *are* the desugaring — `tryCatch()`, `loop()`, `bind()`, `branchFamily()` each produce a specific AST pattern. The Rust compiler does the same thing.

This lives in `barnum_syntax` or a small `barnum_desugar` crate.

### Step 4: Flatten and emit

Flatten is already implemented in Rust (`crates/barnum_ast/src/flat.rs`). The `FlatConfig` is a linear array of 8-byte entries with index-based cross-references. Wire the compiler pipeline into the existing flatten pass and produce the same `FlatConfig` JSON.

### Step 5: CLI

The `barnum` CLI gains `check`, `build`, `run`, `fmt` subcommands for `.barn` files. The existing `run` path for `.barnum.json` files is unchanged.

### Step 6: Tree-sitter grammar

Write a Tree-sitter grammar for `.barn` syntax highlighting. Publish as a standalone package consumed by editor extensions.

### Step 7: LSP server

Implement the language server in Rust, reusing the parser and type checker. The LSP server runs as a long-lived process, re-parsing and re-checking on file changes.

Crate: `barnum_lsp` (new). Dependencies: `barnum_syntax`, `barnum_typecheck`, `tower-lsp` (Rust LSP framework).

### Step 8: Handler stub codegen

`barnum codegen` reads handler declarations from `.barn` files and generates typed handler stubs. Initial target: TypeScript with Zod validators.

## Open questions

1. **Semicolons or newlines?** Pipeline stages are separated by `|>`. But what about sequencing multiple statements within a block? Newlines as statement terminators (like Go, Kotlin) are cleaner than semicolons. But newline sensitivity adds parser complexity (significant whitespace decisions).

2. **Module system.** Can one `.barn` file import types and handlers from another? Probably yes — `import "other.barn" { SomeType, someHandler }`. But this needs a module resolution strategy (relative paths, package names, search paths).

3. **Generics on user-defined types.** The syntax shows `Option<T>` and `Result<T, E>` as built-ins. Can users define their own generic types? Probably yes, but generic type definitions add complexity to the type checker (instantiation, unification, constraint solving).

4. **String interpolation and expressions.** Are inline constants (`_ |> 42`, `_ |> "hello"`) sufficient, or do we need string interpolation (`_ |> "timeout: ${ms}ms"`) and arithmetic expressions? Probably not — computation happens in handlers, not in the workflow language. But error messages and logging might benefit from basic string operations.

5. **How do postfix combinators work in native syntax?** The TS DSL has 41 postfix methods (`.unwrapOr()`, `.mapErr()`, `.map()`, `.filter()`, `.iterate()`, `.collect()`, `.splitFirst()`, `.slice()`, `.take()`, `.skip()`, `.fold()`, `.andThen()`, `.transpose()`, `.or()`, `.flatMap()`, `.isEmpty()`, etc.). Many of these dispatch polymorphically across Option/Result/Iterator via `branchFamily` (two-level dispatch). In the native syntax, these could be:
   - Regular functions: `unwrapOr(throw)`, `mapErr(transform)` — used after `|>`
   - Built-in syntax: `? throw` for unwrapOr (Rust-inspired)
   - Dot-method syntax: `value |> .unwrapOr(throw)` — consistent with `.field` syntax
   - Decided per-combinator (some get syntax, some stay as functions)
   
   The compiler handles the branchFamily dispatch automatically — the user writes `.map(f)` and the compiler emits the correct two-level dispatch based on the type.

6. **Formatter opinions.** Indentation (spaces vs tabs, width). Line wrapping rules for long pipelines. Trailing commas. These are bikeshed decisions but they matter for `barnum fmt` producing consistent output.

7. **Iterator syntax.** The TS DSL uses `branchFamily` to dispatch `.iterate()` across Option/Result/Array inputs. In `.barn` syntax, should Iterator operations be:
   - Explicit: `items |> iterate |> map(f) |> collect`
   - Implicit: type-driven — if the input is an array and you call `.map()`, the compiler auto-wraps in Iterator
   - A mix: explicit entry (`iterate`) but implicit collect when the pipeline expects `T[]`

8. **augment.** The STANDALONE_SYNTAX.md originally proposed an `augment` combinator (run a sub-pipeline, merge output back into original input). This does not exist in the TS DSL. Should it be added as a `.barn` surface construct, or is `bindInput + all + merge` sufficient?
