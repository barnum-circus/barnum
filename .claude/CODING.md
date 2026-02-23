# Rust Coding Patterns

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
