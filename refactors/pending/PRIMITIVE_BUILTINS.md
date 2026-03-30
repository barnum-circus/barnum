# Primitive Builtins: Math, Boolean, String, Array, and Object Operations

## Problem

Barnum's builtins cover structural data transformations (identity, drop, merge, flatten, extractField, pick, tag) and collection-level operations (CollectSome). But it has no primitives for arithmetic, boolean logic, string manipulation, comparisons, or common array/object reshaping.

Without these, trivial operations like "increment a counter", "check if a string is empty", or "concatenate two arrays" require spawning a handler subprocess. That's disproportionate overhead for a one-line computation.

## Design principle: builtins are inline, data-only operations

All builtins execute inline in the Rust scheduler — no subprocess, no IPC. They must be:

1. **Pure and deterministic** — no side effects, no I/O
2. **JSON-serializable** — the AST is data; no closures or function references
3. **Trivial to implement** — one-liner in Rust, not business logic

Builtins are pipeline plumbing. Complex logic belongs in handlers.

## Parameterized vs binary builtins

Two patterns for operations that combine values:

**Parameterized unary** — operates on the pipeline value with a constant parameter baked into the AST node:

```ts
add(5)       // number → number  (pipeline value + 5)
startsWith("http")  // string → boolean
```

BuiltinKind: `{ kind: "Add", value: 5 }`

**Binary** — operates on a tuple from the pipeline (result of `all`):

```ts
pipe(
  all(getPrice, getTax),
  Math.add(),  // [number, number] → number
)
```

BuiltinKind: `{ kind: "Add" }` (no `value` — both operands from pipeline)

**Recommendation**: Support both. Parameterized unary is more common (and more ergonomic). Binary is needed when both operands come from the pipeline.

For unary operations, the parameter is the right-hand operand: `add(5)` means `x + 5`, `gt(10)` means `x > 10`. The pipeline value is always the left-hand operand.

## Proposed builtins by category

### Math

All math builtins operate on `number`. The Rust implementation uses `f64` (serde_json::Number).

#### Unary (pipeline value + constant)

| TypeScript API | Signature | BuiltinKind |
|---|---|---|
| `Math.add(n)` | `number → number` | `{ kind: "Add", value: n }` |
| `Math.sub(n)` | `number → number` | `{ kind: "Sub", value: n }` |
| `Math.mul(n)` | `number → number` | `{ kind: "Mul", value: n }` |
| `Math.div(n)` | `number → number` | `{ kind: "Div", value: n }` |
| `Math.mod(n)` | `number → number` | `{ kind: "Mod", value: n }` |
| `Math.pow(n)` | `number → number` | `{ kind: "Pow", value: n }` |
| `Math.min(n)` | `number → number` | `{ kind: "Min", value: n }` |
| `Math.max(n)` | `number → number` | `{ kind: "Max", value: n }` |

#### Unary (pipeline value only)

| TypeScript API | Signature | BuiltinKind |
|---|---|---|
| `Math.negate()` | `number → number` | `{ kind: "Negate" }` |
| `Math.abs()` | `number → number` | `{ kind: "Abs" }` |
| `Math.floor()` | `number → number` | `{ kind: "Floor" }` |
| `Math.ceil()` | `number → number` | `{ kind: "Ceil" }` |
| `Math.round()` | `number → number` | `{ kind: "Round" }` |

#### Binary (both operands from pipeline)

| TypeScript API | Signature | BuiltinKind |
|---|---|---|
| `Math.add()` | `[number, number] → number` | `{ kind: "Add" }` |
| `Math.sub()` | `[number, number] → number` | `{ kind: "Sub" }` |
| `Math.mul()` | `[number, number] → number` | `{ kind: "Mul" }` |
| `Math.div()` | `[number, number] → number` | `{ kind: "Div" }` |
| `Math.min()` | `[number, number] → number` | `{ kind: "Min" }` |
| `Math.max()` | `[number, number] → number` | `{ kind: "Max" }` |

The binary forms (no parameter) operate on `[number, number]` tuples. The unary forms (with parameter) operate on a single `number`. Overloaded in TypeScript:

```ts
// Overload resolution:
Math.add(5)   // has arg → unary: number → number
Math.add()    // no arg  → binary: [number, number] → number
```

#### Clamp (ternary)

```ts
Math.clamp(min, max)  // number → number
// BuiltinKind: { kind: "Clamp", min: number, max: number }
```

### Boolean

#### Unary

| TypeScript API | Signature | BuiltinKind |
|---|---|---|
| `Bool.not()` | `boolean → boolean` | `{ kind: "Not" }` |

#### Binary (both operands from pipeline)

| TypeScript API | Signature | BuiltinKind |
|---|---|---|
| `Bool.and()` | `[boolean, boolean] → boolean` | `{ kind: "And" }` |
| `Bool.or()` | `[boolean, boolean] → boolean` | `{ kind: "Or" }` |

#### Conditional dispatch

`Bool.branch(trueAction, falseAction)` — dispatch on a boolean value.

Booleans are not tagged unions, so the existing `branch` combinator doesn't apply. Two options:

**Option A: New AST node `IfElse`**

```ts
{ kind: "IfElse", ifTrue: Action, ifFalse: Action }
```

The Rust scheduler checks the boolean and routes to the appropriate branch.

**Option B: Lift boolean to tagged union, reuse `branch`**

```ts
Bool.toTagged()  // boolean → TaggedUnion<{ True: void; False: void }>
// BuiltinKind: { kind: "BoolToTagged" }
```

Then `Bool.branch(trueAction, falseAction)` desugars to:

```ts
pipe(
  Bool.toTagged(),
  branch({ True: pipe(drop(), trueAction), False: pipe(drop(), falseAction) }),
)
```

**Recommendation**: Option B. Reuses existing `branch` machinery. `Bool.branch` is sugar that hides the desugaring. The `BoolToTagged` builtin is trivial:

```rust
fn bool_to_tagged(value: bool) -> Value {
    if value {
        json!({ "kind": "True", "value": null })
    } else {
        json!({ "kind": "False", "value": null })
    }
}
```

### Comparison

All comparisons produce `boolean`.

#### Parameterized (pipeline value vs constant)

| TypeScript API | Signature | BuiltinKind |
|---|---|---|
| `Cmp.eq(v)` | `T → boolean` | `{ kind: "Eq", value: v }` |
| `Cmp.neq(v)` | `T → boolean` | `{ kind: "Neq", value: v }` |
| `Cmp.lt(n)` | `number → boolean` | `{ kind: "Lt", value: n }` |
| `Cmp.lte(n)` | `number → boolean` | `{ kind: "Lte", value: n }` |
| `Cmp.gt(n)` | `number → boolean` | `{ kind: "Gt", value: n }` |
| `Cmp.gte(n)` | `number → boolean` | `{ kind: "Gte", value: n }` |

`Cmp.eq` and `Cmp.neq` use deep JSON equality (`serde_json::Value::eq`). They work on any JSON type.

#### Binary (both operands from pipeline)

| TypeScript API | Signature | BuiltinKind |
|---|---|---|
| `Cmp.eq()` | `[T, T] → boolean` | `{ kind: "Eq" }` |
| `Cmp.lt()` | `[number, number] → boolean` | `{ kind: "Lt" }` |
| etc. | | |

Same overload pattern as Math — with arg is unary, without is binary.

### String

#### Parameterized (pipeline value + constant)

| TypeScript API | Signature | BuiltinKind |
|---|---|---|
| `Str.concat(s)` | `string → string` | `{ kind: "Concat", value: s }` |
| `Str.startsWith(s)` | `string → boolean` | `{ kind: "StartsWith", value: s }` |
| `Str.endsWith(s)` | `string → boolean` | `{ kind: "EndsWith", value: s }` |
| `Str.includes(s)` | `string → boolean` | `{ kind: "Includes", value: s }` |
| `Str.split(sep)` | `string → string[]` | `{ kind: "Split", value: sep }` |
| `Str.replace(pat, rep)` | `string → string` | `{ kind: "Replace", pattern: pat, replacement: rep }` |
| `Str.slice(start, end?)` | `string → string` | `{ kind: "Slice", start: n, end?: n }` |
| `Str.padStart(len, fill?)` | `string → string` | `{ kind: "PadStart", length: n, fill?: s }` |
| `Str.padEnd(len, fill?)` | `string → string` | `{ kind: "PadEnd", length: n, fill?: s }` |

#### Unary (pipeline value only)

| TypeScript API | Signature | BuiltinKind |
|---|---|---|
| `Str.trim()` | `string → string` | `{ kind: "Trim" }` |
| `Str.toUpperCase()` | `string → string` | `{ kind: "ToUpperCase" }` |
| `Str.toLowerCase()` | `string → string` | `{ kind: "ToLowerCase" }` |
| `Str.length()` | `string → number` | `{ kind: "StringLength" }` |
| `Str.isEmpty()` | `string → boolean` | `{ kind: "StringIsEmpty" }` |
| `Str.parseNumber()` | `string → number` | `{ kind: "ParseNumber" }` |
| `Str.parseJson()` | `string → unknown` | `{ kind: "ParseJson" }` |

#### Binary (both operands from pipeline)

| TypeScript API | Signature | BuiltinKind |
|---|---|---|
| `Str.concat()` | `[string, string] → string` | `{ kind: "Concat" }` |

#### Template strings

A template builtin for string interpolation from object fields:

```ts
Str.template("${name} scored ${score} points")
// { name: string; score: number } → string
// BuiltinKind: { kind: "Template", value: "${name} scored ${score} points" }
```

The Rust implementation substitutes `${field}` placeholders from the input object's fields, coercing values to strings. This replaces the common pattern of spawning a handler just to format a string.

### Array

Array builtins operate on `T[]`.

| TypeScript API | Signature | BuiltinKind |
|---|---|---|
| `Arr.length()` | `T[] → number` | `{ kind: "ArrayLength" }` |
| `Arr.isEmpty()` | `T[] → boolean` | `{ kind: "ArrayIsEmpty" }` |
| `Arr.reverse()` | `T[] → T[]` | `{ kind: "Reverse" }` |
| `Arr.first()` | `T[] → T` | `{ kind: "First" }` |
| `Arr.last()` | `T[] → T` | `{ kind: "Last" }` |
| `Arr.take(n)` | `T[] → T[]` | `{ kind: "Take", value: n }` |
| `Arr.skip(n)` | `T[] → T[]` | `{ kind: "Skip", value: n }` |
| `Arr.contains(v)` | `T[] → boolean` | `{ kind: "ArrayContains", value: v }` |
| `Arr.enumerate()` | `T[] → { index: number; value: T }[]` | `{ kind: "Enumerate" }` |
| `Arr.sortBy(field)` | `T[] → T[]` | `{ kind: "SortBy", value: field }` |
| `Arr.unique()` | `T[] → T[]` | `{ kind: "Unique" }` |
| `Arr.zip()` | `[T[], U[]] → [T, U][]` | `{ kind: "Zip" }` |
| `Arr.join(sep)` | `string[] → string` | `{ kind: "Join", value: sep }` |
| `Arr.append()` | `[T[], T[]] → T[]` | `{ kind: "ArrayAppend" }` |

`Arr.first()` and `Arr.last()` panic on empty arrays — this is a Byzantine fault, same as indexing out of bounds. Use `Arr.isEmpty()` + `Bool.branch()` if the array might be empty.

`Arr.sortBy(field)` sorts objects by a string or number field. For simple values, `Arr.sort()` (no field) sorts by natural ordering.

### Object

Object builtins operate on `Record<string, unknown>`.

| TypeScript API | Signature | BuiltinKind |
|---|---|---|
| `Obj.keys()` | `Record<string, T> → string[]` | `{ kind: "Keys" }` |
| `Obj.values()` | `Record<string, T> → T[]` | `{ kind: "Values" }` |
| `Obj.entries()` | `Record<string, T> → { key: string; value: T }[]` | `{ kind: "Entries" }` |
| `Obj.fromEntries()` | `{ key: string; value: T }[] → Record<string, T>` | `{ kind: "FromEntries" }` |
| `Obj.has(key)` | `Record<string, T> → boolean` | `{ kind: "Has", value: key }` |
| `Obj.omit(...keys)` | `T → Omit<T, keys>` | `{ kind: "Omit", value: keys }` |
| `Obj.set(key, value)` | `T → T & { [key]: value }` | `{ kind: "Set", key: k, value: v }` |
| `Obj.size()` | `Record<string, T> → number` | `{ kind: "ObjectSize" }` |

`Obj.omit` is the complement of `pick`. `Obj.set` adds or overwrites a single field with a constant value — useful for tagging objects with metadata without a full handler.

### Type conversions

| TypeScript API | Signature | BuiltinKind |
|---|---|---|
| `Convert.toString()` | `T → string` | `{ kind: "ToString" }` |
| `Convert.toNumber()` | `string → number` | `{ kind: "ToNumber" }` |
| `Convert.toBool()` | `T → boolean` | `{ kind: "ToBool" }` |
| `Convert.toJson()` | `T → string` | `{ kind: "ToJson" }` |
| `Convert.fromJson()` | `string → unknown` | `{ kind: "FromJson" }` |

`Convert.toBool()` uses JavaScript truthiness rules: `false`, `0`, `""`, `null`, `undefined` → false, everything else → true.

`Convert.toJson()` / `Convert.fromJson()` are `JSON.stringify` / `JSON.parse`.

## Namespace organization

All primitive builtins live in namespace objects, matching the `Option` pattern:

```ts
import { Math, Bool, Cmp, Str, Arr, Obj, Convert } from "@barnum/barnum/builtins";

pipe(
  constant({ items: [1, 2, 3] }),
  extractField("items"),
  Arr.length(),             // 3
  Math.mul(2),              // 6
  Cmp.gt(5),                // true
  Bool.branch(
    constant("many"),
    constant("few"),
  ),
)
```

The namespace names are short: `Math`, `Bool`, `Cmp`, `Str`, `Arr`, `Obj`. They won't collide with globals because they're imported, not ambient.

**Shadow concern**: `Math` shadows the global `Math` object. Options:

1. Use `Num` instead of `Math` to avoid the shadow
2. Keep `Math` — it's a named import, not global; users who need global `Math` can alias
3. Export as `BarnumMath` or `math` (lowercase)

**Recommendation**: Use `Num` to avoid confusion. The global `Math` object is used frequently and shadowing it invites bugs.

## Postfix methods

High-frequency operations could also be postfix methods on TypedAction:

```ts
// Instead of:
pipe(handler, Num.add(1))
// Could be:
handler.add(1)
```

**Recommendation**: Defer postfix methods for primitive builtins. The namespace form is clear and discoverable. Postfix methods on TypedAction are already used for structural operations (`.branch()`, `.flatten()`, `.drop()`, `.get()`, `.pick()`). Adding math/string/boolean postfix methods would bloat the TypedAction interface and blur the line between structural and computational.

## Error handling in builtins

What happens when a builtin fails at runtime?

- `Num.div(0)` — division by zero
- `Arr.first()` on empty array
- `Str.parseNumber()` on non-numeric string
- `Convert.fromJson()` on malformed JSON

These are Byzantine faults. The builtin's type signature promises a result. If the runtime value violates the type assumption, the AST's invariants are broken. The scheduler should panic the workflow (same as any other invariant violation).

For fallible operations, provide `Option`-returning variants:

| Fallible | Safe variant | Signature |
|---|---|---|
| `Arr.first()` | `Arr.tryFirst()` | `T[] → Option<T>` |
| `Arr.last()` | `Arr.tryLast()` | `T[] → Option<T>` |
| `Str.parseNumber()` | `Str.tryParseNumber()` | `string → Option<number>` |
| `Convert.fromJson()` | `Convert.tryFromJson()` | `string → Option<unknown>` |
| `Num.div(n)` | `Num.tryDiv(n)` | `number → Option<number>` |

The `try` variants return `Option<T>`, letting the pipeline handle failure via `Option.unwrapOr`, `Option.map`, etc. No panics.

## Interaction with let-bindings

Many of these builtins become more useful with let-bindings (LET_BINDINGS.md). Without them, combining two pipeline values requires `all` + a binary builtin:

```ts
// "total = price * quantity" without let-bindings:
pipe(
  all(extractField("price"), extractField("quantity")),
  Num.mul(),  // binary form
)

// With let-bindings:
let_(({ price, quantity }) =>
  Num.mul(price, quantity)  // or some expression form
)
```

The binary overload pattern (no-arg for tuple input, with-arg for constant) works without let-bindings. Let-bindings would make complex expressions more readable but aren't a prerequisite.

## Implementation in Rust

Each `BuiltinKind` variant maps to a match arm in the Rust scheduler's builtin executor. The implementation is straightforward — each operation is 1-5 lines of Rust operating on `serde_json::Value`:

```rust
match &builtin {
    BuiltinKind::Add { value: Some(n) } => {
        // Unary: pipeline_value + n
        let x = pipeline_value.as_f64().expect("Add: expected number");
        Value::from(x + n)
    }
    BuiltinKind::Add { value: None } => {
        // Binary: pipeline_value[0] + pipeline_value[1]
        let arr = pipeline_value.as_array().expect("Add: expected [number, number]");
        let a = arr[0].as_f64().expect("Add: expected number");
        let b = arr[1].as_f64().expect("Add: expected number");
        Value::from(a + b)
    }
    // ...
}
```

The `expect` calls produce Byzantine fault panics — the workflow dies with a clear message. This is correct: if a `number → number` builtin receives a string, the AST's type guarantees are broken.

## Priority

### Tier 1: needed for basic pipeline logic

- **Comparison**: `Cmp.eq`, `Cmp.gt`, `Cmp.lt`, `Cmp.gte`, `Cmp.lte`, `Cmp.neq`
- **Boolean**: `Bool.not`, `Bool.branch`
- **Math**: `Num.add`, `Num.sub`, `Num.mul`
- **Array**: `Arr.length`, `Arr.isEmpty`, `Arr.first`, `Arr.last`, `Arr.join`
- **String**: `Str.length`, `Str.isEmpty`, `Str.concat`, `Str.includes`, `Str.template`
- **Object**: `Obj.omit`, `Obj.set`, `Obj.has`

### Tier 2: useful for data shaping

- **Math**: `Num.div`, `Num.mod`, `Num.min`, `Num.max`, `Num.clamp`, `Num.abs`, `Num.negate`, `Num.floor`, `Num.ceil`, `Num.round`
- **Array**: `Arr.take`, `Arr.skip`, `Arr.reverse`, `Arr.enumerate`, `Arr.sortBy`, `Arr.unique`, `Arr.zip`, `Arr.append`, `Arr.contains`
- **String**: `Str.split`, `Str.trim`, `Str.toUpperCase`, `Str.toLowerCase`, `Str.replace`, `Str.slice`, `Str.startsWith`, `Str.endsWith`
- **Object**: `Obj.keys`, `Obj.values`, `Obj.entries`, `Obj.fromEntries`, `Obj.size`
- **Convert**: `Convert.toString`, `Convert.toNumber`, `Convert.toJson`, `Convert.fromJson`

### Tier 3: safe variants

- `Arr.tryFirst`, `Arr.tryLast`, `Str.tryParseNumber`, `Convert.tryFromJson`, `Num.tryDiv`

### Tier 4: binary overloads

- Binary forms of all Math and Comparison builtins (for `all` → combine patterns)

## Open questions

### Naming: `Math` vs `Num`

`Math` matches the JS global but shadows it. `Num` is shorter and avoids the shadow. Leaning toward `Num`.

### Should binary forms exist?

Binary forms (`Num.add()` operating on `[number, number]`) add API surface. The same thing is achievable with `all(a, b)` → parameterized form. But that requires an intermediate `extractIndex` step. Binary forms are cleaner for the `all(a, b) → combine` pattern.

### Should `Obj.set` exist?

`Obj.set("status", "done")` adds a constant field. This overlaps with `augment(constant({ status: "done" }))`. The augment form is more general (can compute the value). `Obj.set` is more readable for the constant case. Both could exist.

### Should `Str.template` exist?

Template strings are powerful but add complexity to the Rust implementation (string interpolation with type coercion). The alternative is a handler that does string formatting. But templates are so common in workflow orchestration (constructing messages, URLs, paths) that a builtin feels justified.

### `Bool.branch` vs `IfElse` AST node

Desugaring `Bool.branch` to `BoolToTagged + branch` works but adds an intermediate conversion step. A dedicated `IfElse` AST node is simpler at runtime. The desugaring approach is more principled (reuses `branch`). Either works.
