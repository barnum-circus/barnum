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
type Shape =
  | Circle(Float)
  | Rect({ width: Float, height: Float })
  | Point                                   // unit variant (value = void)

// Built-ins, always in scope:
//   Option<T>     = Some(T) | None
//   Result<T, E>  = Ok(T) | Err(E)
//   LoopResult<C, B> = Continue(C) | Break(B)
```

Types are structural for objects and nominal for tagged unions. An object `{ path: String, content: String }` matches any handler expecting those fields. A tagged union `Shape` is distinct from another union with the same variants — the union name matters for exhaustiveness checking.

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

Builtins are the ALU operations that run inline in Rust. In the native syntax, most have dedicated syntax rather than function calls.

```barnum
// Field access — desugars to GetField
value |> .name              // extract "name" from the pipeline value

// Indexing — desugars to GetIndex
value |> .[0]               // extract index 0

// Pick — desugars to Pick
value |> .{name, age}       // select fields "name" and "age"

// Tag — desugars to Tag (wrapping a value as a tagged union variant)
value |> @Ok                // wrap as { kind: "Ok", value: value }

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
```

The `.field`, `.[index]`, `.{fields}` syntax replaces the TS DSL's `.getField("field")`, `getIndex(n)`, `.pick("a", "b")`. The `@Variant` syntax replaces `.tag("Variant")`. The `_` discard replaces `.drop()`.

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

`let` introduces concurrent bindings that are available as named references throughout the body. Desugars to the same Handle/Perform/All structure as the TS DSL's `bind`.

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

`recur` and `done` are scoped effect tokens. `recur` restarts the loop body with a new input. `done` exits the loop with the break value. Desugars to Handle/Perform/Branch, same as the TS DSL.

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

### Augment

```barnum
// Run a sub-pipeline, merge its output back into the original input
augment(.name |> enrichName)
// Input: { name: String, age: Number }
// enrichName: String -> { displayName: String }
// Output: { name: String, age: Number, displayName: String }
```

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

### Desugaring

Most surface syntax is sugar over Handle/Perform:

| Surface syntax | Desugars to |
|---|---|
| `try (t) { body } catch { recovery }` | `Handle(eid, body, Chain(GetField("payload"), Chain(recovery, Tag("Discard"))))` |
| `loop (r, d) { body }` | `Chain(Tag("Continue"), Handle(eid, Branch({ Continue: body, Break: Identity }), RestartBodyHandler))` |
| `earlyReturn (e) { body }` | Same as loop but the body is wrapped differently |
| `let x = a, y = b in { body }` | `Chain(All(a, b, Identity), Handle(e0, readVar(0), Handle(e1, readVar(1), Chain(GetIndex(2), body))))` |
| `match { A => x, B => y }` | `Branch({ A: Chain(GetField("value"), x), B: Chain(GetField("value"), y) })` |
| `.field` | `Invoke(Builtin(GetField("field")))` |
| `@Variant` | `Invoke(Builtin(Tag("Variant")))` |
| `_` | `Invoke(Builtin(Drop))` |
| `42` (literal) | `Invoke(Builtin(Constant(42)))` |

The desugaring is mechanical and produces the same AST the TS DSL would produce. The compiler and engine share the `barnum_ast` crate.

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
  outputValidator: z.union([
    z.object({ kind: z.literal("Ok"), value: z.void() }),
    z.object({ kind: z.literal("Err"), value: z.string() }),
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

Lower the surface AST to the core `Action` enum. This is mechanical: each surface construct (`try`/`catch`, `loop`, `let`/`in`, etc.) maps to a fixed pattern of Handle/Perform/Chain/Branch/All nodes. The TS DSL's combinator functions *are* the desugaring — `tryCatch()`, `loop()`, `bind()` each produce a specific AST pattern. The Rust compiler does the same thing.

This lives in `barnum_syntax` or a small `barnum_desugar` crate.

### Step 4: Flatten and emit

Reuse the existing `flatten()` logic (or port it to Rust if it's currently TS-only). Produce the same `FlatConfig` JSON. Write it to a file.

If flatten is currently in TypeScript: port to Rust in `barnum_ast` (it's a pure tree-to-table transformation with no I/O).

If flatten is already in Rust: wire it into the compiler pipeline.

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

5. **How do postfix combinators work in native syntax?** The TS DSL has `.unwrapOr(action)`, `.mapErr(action)`, `.mapOption(action)`. In the native syntax, these could be:
   - Regular functions: `unwrapOr(throw)`, `mapErr(transform)` — used after `|>`
   - Built-in syntax: `? throw` for unwrapOr (Rust-inspired)
   - Decided per-combinator (some get syntax, some stay as functions)

6. **Formatter opinions.** Indentation (spaces vs tabs, width). Line wrapping rules for long pipelines. Trailing commas. These are bikeshed decisions but they matter for `barnum fmt` producing consistent output.
