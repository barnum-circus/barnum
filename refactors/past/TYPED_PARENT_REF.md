# Typed ParentRef

## Motivation

`deliver` has a nested dispatch: first on ParentRef variant (SingleChild vs IndexedChild), then on FrameKind (Chain/Loop or All/ForEach). The outer dispatch doesn't determine the code path — the inner FrameKind match does. The `unreachable!()` arms exist because ParentRef is too coarse: SingleChild could point at a Chain, Loop, or (after effects) Handle frame, but only one of those is valid at each call site.

One ParentRef variant per frame kind eliminates the inner dispatch. The ParentRef variant determines the code path. FrameKind destructuring becomes an assertion, not control flow.

## Current state

`crates/barnum_engine/src/frame.rs`:

```rust
pub enum ParentRef {
    SingleChild { frame_id: FrameId },
    IndexedChild { frame_id: FrameId, child_index: usize },
}
```

`crates/barnum_engine/src/lib.rs`, `deliver`:

```rust
match parent_ref {
    ParentRef::SingleChild { .. } => {
        let frame = self.frames.remove(frame_id).expect("parent frame exists");
        match frame.kind {
            FrameKind::Chain { rest } => { /* trampoline */ }
            FrameKind::Loop { body } => { /* Continue/Break */ }
            _ => unreachable!("SingleChild parent must be Chain or Loop, got {:?}", frame.kind),
        }
    }
    ParentRef::IndexedChild { child_index, .. } => {
        let frame = self.frames.get_mut(frame_id).expect("parent frame exists");
        match &mut frame.kind {
            FrameKind::All { results } | FrameKind::ForEach { results } => {
                /* fill slot, join if complete */
            }
            _ => unreachable!("IndexedChild parent must be All or ForEach, got {:?}", frame.kind),
        }
    }
}
```

`advance` constructs ParentRef at four sites:

| Frame kind | ParentRef | Line |
|---|---|---|
| Chain | `SingleChild { frame_id }` | `lib.rs:298` |
| All | `IndexedChild { frame_id, child_index: i }` | `lib.rs:322` |
| ForEach | `IndexedChild { frame_id, child_index: i }` | `lib.rs:352` |
| Loop | `SingleChild { frame_id }` | `lib.rs:384` |

## Proposed

### ParentRef enum

```rust
#[derive(Debug, Clone, Copy)]
pub enum ParentRef {
    Chain { frame_id: FrameId },
    Loop { frame_id: FrameId },
    All { frame_id: FrameId, child_index: usize },
    ForEach { frame_id: FrameId, child_index: usize },
}

impl ParentRef {
    #[must_use]
    pub const fn frame_id(self) -> FrameId {
        match self {
            Self::Chain { frame_id }
            | Self::Loop { frame_id }
            | Self::All { frame_id, .. }
            | Self::ForEach { frame_id, .. } => frame_id,
        }
    }
}
```

### deliver

Each ParentRef variant maps to one code path. FrameKind destructuring is an assertion — the ParentRef variant guarantees which FrameKind the frame holds.

```rust
fn deliver(
    &mut self,
    parent: Option<ParentRef>,
    value: Value,
) -> Result<Option<Value>, CompleteError> {
    let Some(parent_ref) = parent else {
        return Ok(Some(value));
    };

    match parent_ref {
        ParentRef::Chain { frame_id } => {
            let frame = self.frames.remove(frame_id).expect("parent frame exists");
            let FrameKind::Chain { rest } = frame.kind else {
                unreachable!("Chain ParentRef points to non-Chain frame: {:?}", frame.kind)
            };
            self.advance(rest, value, frame.parent)?;
            Ok(None)
        }

        ParentRef::Loop { frame_id } => {
            let frame = self.frames.remove(frame_id).expect("parent frame exists");
            let FrameKind::Loop { body } = frame.kind else {
                unreachable!("Loop ParentRef points to non-Loop frame: {:?}", frame.kind)
            };
            match value["kind"].as_str() {
                Some("Continue") => {
                    let frame_id = self.insert_frame(Frame {
                        parent: frame.parent,
                        kind: FrameKind::Loop { body },
                    });
                    self.advance(
                        body,
                        value["value"].clone(),
                        Some(ParentRef::Loop { frame_id }),
                    )?;
                    Ok(None)
                }
                Some("Break") => self.deliver(frame.parent, value["value"].clone()),
                _ => Err(CompleteError::InvalidLoopResult { value }),
            }
        }

        ParentRef::All { frame_id, child_index }
        | ParentRef::ForEach { frame_id, child_index } => {
            let frame = self.frames.get_mut(frame_id).expect("parent frame exists");
            let results = match &mut frame.kind {
                FrameKind::All { results } | FrameKind::ForEach { results } => results,
                other => unreachable!(
                    "All/ForEach ParentRef points to wrong frame: {:?}", other
                ),
            };
            results[child_index] = Some(value);
            if results.iter().all(Option::is_some) {
                let collected: Vec<Value> =
                    results.iter_mut().map(|r| r.take().unwrap()).collect();
                let parent = frame.parent;
                self.frames.remove(frame_id);
                self.deliver(parent, Value::Array(collected))
            } else {
                Ok(None)
            }
        }
    }
}
```

The All/ForEach arms combine with `|` because their delivery logic is identical (fill slot, join when complete). The FrameKind match inside is an assertion: All ParentRef guarantees All frame, ForEach ParentRef guarantees ForEach frame.

### advance

Four construction sites, one-to-one replacement:

```rust
// Chain (was SingleChild)
self.advance(first, value, Some(ParentRef::Chain { frame_id }))?;

// All (was IndexedChild)
self.advance(child, value.clone(), Some(ParentRef::All { frame_id, child_index: i }))?;

// ForEach (was IndexedChild)
self.advance(body, element, Some(ParentRef::ForEach { frame_id, child_index: i }))?;

// Loop (was SingleChild)
self.advance(body, value, Some(ParentRef::Loop { frame_id }))?;
```

### Tests

No test logic changes. Tests don't construct or inspect ParentRef — it's internal to the engine. The existing test suite validates the refactor.

## Interaction with effects (Phase 1)

The effects design doc adds a Handle variant to ParentRef with a nested `HandleSide` enum distinguishing body children from handler children. That variant slots into this typed enum naturally — it's just one more arm in `deliver` and one more match pattern in `frame_id()`. The typed ParentRef design was chosen specifically to make this extension clean.
