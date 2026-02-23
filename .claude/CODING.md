# Rust Coding Patterns

## Design philosophy

**Make impossible states unrepresentable.** If you find yourself reaching for a `HashSet` to track "which IDs are special" or a boolean to distinguish cases, stop. That's a sign the types are wrong. Use newtypes or enums so the compiler enforces the distinction.

**Step back during refactors.** If mid-refactor you discover a shortcut is needed (parallel tracking structures, runtime checks for things that could be static), pause and reconsider the design. It's faster to get the types right now than to do another pass later. The correct primitives make the code fall out naturally.

## Anti-patterns

- `unwrap()` when `if let` or `?` would work
- Overly generic type signatures that obscure intent
- Closures when traits would be clearer
- Comments explaining what instead of why
- Check-then-act: `if map.contains_key(&k) { ... map.insert(k, v) }` — use `entry` API instead

## Patterns to look for

**One-variant enums are fine** when a second variant is coming. See `Transport` in `daemon/io.rs`. When the new variant lands, callers already handle the enum.

**Structural duplication.** If two data structures have the same shape but different types, unify them with generics and associated types. See `TransportMap<Id: TransportId>` in `daemon/io.rs`.

**Entry API over check-then-act.** Instead of:
```rust
if !map.contains_key(&key) {
    map.insert(key, value);
}
```

Use:
```rust
use std::collections::hash_map::Entry;
let Entry::Vacant(entry) = map.entry(key) else { return };
entry.insert(value);
```
