# Suspend-via-effect: serialize the engine to a file and resume from it

Engine-side. Requires `barnum_engine` changes. Companion to `RESUMPTION.md`,
which it supersedes for the "how do we actually halt and resume" question —
`RESUMPTION.md` proposes checkpointing after *every* completion step; this doc
proposes halting at *one specific point* the workflow author chooses, via an
effect.

## The idea being evaluated

> Have an algebraic effect handler that, when handled on the Rust side, stops
> everything and serializes the whole engine to a file. That file is handed to
> the effect handler. Later we resume from that file.

Restated in the engine's own vocabulary: a workflow performs a `Suspend`
effect. The effect bubbles to a handler. Instead of running user code, the
handler serializes `WorkflowState` to disk and terminates the process. A later
invocation reads the file, reconstructs `WorkflowState`, and continues the
event loop as if the completion had just arrived.

This is feasible, and it is a better fit for Barnum than the
"checkpoint-after-every-step" model in `RESUMPTION.md`, for one reason: Barnum
*already has* algebraic effect handlers (`ResumeHandle`/`RestartHandle`), and
the engine is *already* a pure, I/O-free state machine. The mechanism the user
is describing is mostly already built. What's missing is (1) serializability of
the frame arena and (2) a way for an effect to mean "stop the loop" rather than
"dispatch a handler."

## Why this is more tractable than it sounds

The hard part of process suspension is usually capturing the call stack. Barnum
doesn't have that problem. Re-read `barnum_engine/src/lib.rs:1-5`:

> The engine is a synchronous state machine with no I/O, no async, no timers,
> and no concurrency.

The entire resumable state of a workflow is the six fields of `WorkflowState`
(`lib.rs:170-181`):

```rust
pub struct WorkflowState {
    flat_config: FlatConfig,                  // static, already serializable
    frames: Arena<Frame>,                     // the "call stack" — see below
    task_to_frame: BTreeMap<TaskId, FrameId>, // which tasks are in flight
    pending_effects: VecDeque<PendingEffect>, // queued dispatches/restarts
    next_task_id: u32,                        // scalar
    terminal_value: Option<Value>,            // scalar/Value
}
```

There is no hidden continuation, no native stack, no closure to capture. The
"stack" is the `frames` arena — an explicit, reified tree of `Frame` values
(`frame.rs:157-164`), each holding a `ParentRef` and a `FrameKind`. Every
`FrameKind` payload is already plain data: `ActionId`/`HandlerId` (u32
newtypes), `Vec<Option<Value>>`, `serde_json::Value`, `ResumeHandlerId`/
`RestartHandlerId` (u16 newtypes). The frame tree is a snapshot of execution.
Snapshotting it *is* snapshotting the workflow.

The async lives entirely in `barnum_event_loop::Scheduler` (`lib.rs:82-91`):
the tokio channel, the subprocess spawning, the process group. None of that is
engine state — it's reconstructed fresh on resume by building a new
`Scheduler`. So the suspend/resume boundary falls cleanly on the
engine/event-loop seam that already exists.

## What blocks direct serialization today

Two things, both concrete and small.

### 1. `FrameId` is a generational arena index, not data

`FrameId = thunderdome::Index` (`frame.rs:10`). It is a `(u32 slot, u32
generation)` pair that is meaningful only relative to one live `Arena`
instance. `thunderdome::Index` does not implement `Serialize`, and even if it
did, the generations are an implementation detail of the specific arena that
produced them. `task_to_frame`, every `ParentRef`, and
`ResumePerformFrame::resume_handle_frame_id` all reference frames by `FrameId`,
so the arena can't be naively serialized.

This is the only structural obstacle, and it has a clean fix: serialize the
arena to a representation that uses stable, self-contained frame keys, and
rebuild a fresh arena on load, translating old keys to new `Index` values via a
map. thunderdome exposes `Arena::iter()` (yields `(Index, &T)`) and the `Index`
type can be decomposed into `slot()`/`generation()`. The translation is
mechanical:

- On save: assign each live frame a dense `u32` serial id in iteration order,
  build `old_index -> serial` map, rewrite every `FrameId`-bearing field
  (`Frame::parent`, `task_to_frame` values, `ResumePerformFrame`,
  any future frame-referencing field) to serials, emit
  `Vec<(serial, SerializableFrame)>`.
- On load: insert each `SerializableFrame` into a new `Arena`, build
  `serial -> new_index` map, rewrite all serials back to the new `Index`es.

No generational reuse problem on load because every serial maps to exactly one
fresh insert. The stale-`FrameId` invariant tested in
`frame.rs:174-196` is preserved: a resumed arena is a brand-new arena with
brand-new generations; old serialized indices never collide with it because
they were never `Index`es to begin with.

#### You do **not** make the engine generic over the frame-id type

The tempting-but-wrong version of this is to parameterize `Frame`,
`ParentRef`, `WorkflowState`, etc. over `TFrameId` so the live engine uses
`thunderdome::Index` and the serialized form uses `u32`. That infects every
type and every function signature in the engine with a type parameter that
exists only to satisfy serialization. It's the "fucking annoying" outcome, and
it's unnecessary.

The serial-id form is a **separate, parallel set of types** that exists only at
the serialization boundary — it is not the engine's runtime representation:

```rust
// Runtime types stay exactly as they are: FrameId = thunderdome::Index.
// These snapshot types are the on-disk shape, and nothing else uses them.
#[derive(Serialize, Deserialize)]
struct WorkflowSnapshot {
    flat_config: FlatConfig,
    frames: Vec<SnapshotFrame>,           // dense, index = serial id
    task_to_frame: Vec<(TaskId, u32)>,    // u32 = serial id
    next_task_id: u32,
    terminal_value: Option<Value>,
    pending_effects: Vec<SnapshotEffect>,
}

#[derive(Serialize, Deserialize)]
struct SnapshotFrame {
    parent: Option<SnapshotParentRef>,    // ParentRef with u32 instead of FrameId
    kind: SnapshotFrameKind,              // FrameKind with u32 instead of FrameId
}
```

`to_snapshot(&self) -> WorkflowSnapshot` and `from_snapshot(WorkflowSnapshot)
-> WorkflowState` are the only two places that know both representations and do
the `Index <-> u32` translation. Everything inside the engine keeps using
`thunderdome::Index` and never sees `WorkflowSnapshot`.

The cost is the duplicated shape: `SnapshotParentRef`/`SnapshotFrameKind` mirror
`ParentRef`/`FrameKind` with the one field type swapped. That duplication is the
honest price, and it's the *right* trade — it's mechanical, it's confined to one
module, and it keeps the type parameter out of the entire engine. Most fields
(`Vec<Option<Value>>`, the `*HandlerId` newtypes, `ActionId`, `RestartHandleSide`)
are identical between the two and can be reused directly; only the `FrameId`
fields differ. If the mirror ever drifts out of sync with the runtime types, the
translation functions stop compiling — the duplication is compiler-checked, not
a silent footgun.

(An alternative that avoids the mirror types entirely: a `#[serde(with = ...)]`
adapter on `FrameId` fields that serializes an `Index` via its
`slot`/`generation`, plus a custom `Arena` (de)serializer. This is rejected:
serialized generations are meaningless across arena instances, so you'd be
persisting implementation noise and still need a load-time rebuild. The serial-id
mirror is cleaner — it persists *meaning*, not arena internals.)

### 2. There is no "suspend" effect kind

The engine has two effect families today (`frame.rs:80-120`):

- `ResumeHandle`/`ResumePerform` — handler runs inline at the perform site,
  produces `[value, new_state]`, never suspends the surrounding work.
- `RestartHandle`/`RestartPerform` — body is torn down, handler runs, handler
  output becomes the new body input.

Neither means "stop the event loop and hand control to the host." `Suspend` is
a *third* effect shape: when performed, it does not advance any handler DAG; it
drains to a terminal signal that the event loop interprets as "serialize and
exit," carrying a payload (e.g. a filename, or a tag the host maps to a file).

The cleanest framing — and the one that matches "the file is passed to the
effect handler" — is that `Suspend` is the engine producing a *new kind of
pending effect* the loop must handle, parallel to `Dispatch` and `Restart`
(`lib.rs:79-84`):

```rust
pub enum PendingEffectKind {
    Dispatch(DispatchEvent),
    Restart(RestartEvent),
    Suspend(SuspendEvent),   // NEW
}
```

`SuspendEvent` carries the perform-site payload (the `Value` the workflow
passed to the `Suspend` perform) and the `FrameId` of the perform site, so on
resume we know where the suspended computation is waiting for its result.

## How suspend/resume threads through the existing loop

`run_workflow` (`barnum_event_loop/src/lib.rs:314-427`) already has the exact
shape needed. It pops pending effects, and for each one matches on kind. Adding
a `Suspend` arm is a local change:

```rust
EventKind::Suspend(suspend_event) => {
    let snapshot = workflow_state.to_snapshot();           // serial-id translation
    let bytes = serde_json::to_vec(&snapshot)?;            // or bincode
    // "the file is passed to the handler": the host decides the path.
    on_suspend(suspend_event.payload, &bytes)?;            // host callback
    return Ok(SuspendOutcome::Suspended);                  // stop the loop
}
```

The return type of `run_workflow` widens from `Result<Value, _>` to
`Result<RunOutcome, _>` where `RunOutcome` is a two-variant enum
(`Completed(Value)` / `Suspended` — per the project rule preferring named
two-variant enums over booleans). The host (CLI) maps `Suspended` to a
process exit; `Completed` to printing the result as today.

Resume is the mirror image and reuses everything:

```rust
let snapshot: WorkflowSnapshot = serde_json::from_slice(&bytes)?;
let mut workflow_state = WorkflowState::from_snapshot(snapshot); // rebuild arena
let mut scheduler = Scheduler::new(/* fresh */);
// The perform site is still parked in `frames`, waiting for a Value.
// Resuming means: deliver that Value as if a completion arrived.
resume_workflow(&mut workflow_state, &mut scheduler, resume_value).await
```

The key realization: **a suspended workflow is structurally identical to a
workflow waiting on an in-flight handler.** The perform-site frame is parked in
the arena exactly the way an `Invoke` frame is parked while its subprocess
runs. The only difference is that no tokio task will ever deliver its
completion — the *host* delivers it on resume, by feeding `resume_value` to
`complete()` against the parked perform site. So `resume_workflow` is
`run_workflow` with one extra initial step: inject the resume completion, then
enter the normal loop.

This is why the model fits so well. We are not inventing a resumption path. We
are reusing the completion path, with the host playing the role the scheduler
plays for ordinary handlers.

## In-flight handlers at suspend time: kill and re-invoke

`Suspend` is legal at any time, including when sibling branches
(parallel/forEach) have handlers mid-flight. When the process exits, those
tokio tasks die — their subprocesses are killed via the process group
(`barnum_event_loop/src/lib.rs:71-91`), and their results are never collected.
On resume, their `Invoke` frames are still parked in `frames` and
`task_to_frame`, but the tasks that would have completed them no longer exist.

**The design: kill in-flight handlers at suspend, re-invoke them on resume.**
On resume, walk `task_to_frame`, and for each parked `Invoke` frame, re-dispatch
it — same handler, same input — producing a fresh `TaskId` and a fresh
subprocess. The workflow continues as if those handlers had simply taken a very
long time to run.

This imposes one requirement on the workflow author and one on the engine:

- **Author:** handlers that may be in flight across a suspend must be
  idempotent, because a killed-then-re-invoked handler runs its side effects
  twice (once before the kill, fully on re-invoke). This is the same idempotency
  contract `RESUMPTION.md` already names, and it's the contract `README.md`'s
  long-lived-agent model already implies (a handler that creates a PR checks
  whether the PR exists first).

- **Engine:** re-invoke needs the input value each parked `Invoke` was
  dispatched with. Today the `Invoke` frame stores only `handler: HandlerId`
  (`frame.rs:101-104`) — the input value is consumed by `dispatch` and not
  retained. Re-invoke is impossible without it. **The `Invoke` frame must also
  store the value it was dispatched with.** This is a prerequisite change, lands
  as its own sub-refactor, and is independently sensible: it makes the frame
  tree fully self-describing (the comment at `frame.rs:99-100` already justifies
  storing `handler` on the frame "so the frame tree is self-describing for
  observability" — the input value belongs there for the same reason).

Re-invoke reuses the dispatch path verbatim. On resume, after `from_snapshot`
rebuilds the arena, the resume routine iterates `task_to_frame`, reads each
`Invoke` frame's stored `(handler, value)`, and hands it to
`scheduler.dispatch`. No new engine code path — the same `DispatchEvent` the
engine emits during normal advance.

### Why kill-and-re-invoke over the alternatives

Two other models were considered and deferred:

- **Restart to a boundary** — tear the in-flight subtree down to its nearest
  `RestartHandle` and re-advance the body. Rejected because a parked `Invoke`
  deep inside a `Chain`/`All` often has *no* enclosing restart boundary, so
  there's nothing to restart to. It would force every suspendable workflow to be
  wrapped in restart handlers. Per-`Invoke` re-invoke has no such requirement —
  every parked `Invoke` knows its own handler and input, so it can always
  re-run in place.

- **Let them complete** — instead of killing in-flight handlers at suspend,
  drain them first: stop dispatching new work, wait for the outstanding tokio
  tasks to finish and their completions to land, *then* snapshot. This avoids
  the double-run entirely (no handler is interrupted), so it needs no
  idempotency for the in-flight set. It's a real and arguably nicer end state,
  but it's a deferred follow-on, not the first version: it complicates the
  suspend path (the loop must enter a drain mode, refuse new dispatches, and
  only snapshot once `task_to_frame` is empty of the relevant tasks), and it
  delays suspend by however long the slowest in-flight handler takes — which,
  for the agent workloads Barnum targets, can be many minutes. **Killing them is
  the right first version:** suspend is immediate, the engine change is just
  "store the input on the `Invoke` frame," and the idempotency contract is one
  the framework already demands. Drain-then-suspend can be added later as an
  opt-in for workflows that can't tolerate double-runs.

## Where suspend differs from `RESUMPTION.md`

`RESUMPTION.md` proposes serializing `WorkflowState` *after every completion*
as a crash-recovery checkpoint, and resuming from the last checkpoint on
startup. That's a durability mechanism: it survives crashes the workflow didn't
ask for.

This doc proposes serializing *only when the workflow performs a `Suspend`
effect* — a cooperative, author-chosen halt. The two compose; they are not
alternatives:

- `Suspend` gives the workflow author a first-class "pause here, resume later
  with this value" primitive — e.g. wait for a human approval that may arrive
  days later, with the process not running in between. That is exactly the
  long-lived-agent use case in `README.md`.
- Checkpoint-after-every-step gives crash recovery for free once
  serialization exists.

**Both need the arena-serialization work.** That work is the irreducible
core and should land first as its own sub-refactor (see below). Once
`WorkflowState` round-trips through bytes, `Suspend` and
checkpoint-on-every-step are independent features built on top.

## The userland surface

`Suspend` should appear as a postfix-composable effect, consistent with the
existing effect handlers in `libs/barnum/src` (`try-catch.ts`, `bind.ts`,
`recursive.ts`, `retry.ts` all wrap `ResumeHandle`/`RestartHandle`). Sketch:

```ts
// Perform site: pause the workflow, surfacing `value` to the host.
// Resolves (on resume) to whatever value the host feeds back in.
const approved: boolean = await suspend(approvalRequest);
```

The host side is a function, not a workflow combinator — it is the thing that
owns the file:

```ts
const outcome = await compiled.run();
if (outcome.kind === "suspended") {
  fs.writeFileSync("checkpoint.barnum", outcome.snapshot);
  // ... days later, in a new process ...
}
const resumed = await CompiledWorkflow.resume(
  fs.readFileSync("checkpoint.barnum"),
  /* resumeValue */ true,
);
```

Note this keeps the `PRECOMPILATION.md` "framework owns no file I/O" rule: the
engine produces bytes, the host writes/reads the file. "The file is passed to
the handler" from the original framing becomes "the bytes are passed to the
host callback, which decides where they live."

## Open questions

1. **Serialization format.** `serde_json` is the obvious first cut (every value
   in the state is already a `serde_json::Value`, and `FlatConfig` already
   round-trips through JSON per `PRECOMPILATION.md`). `bincode` is smaller/faster
   but adds a dependency and loses inspectability. Recommendation: JSON first,
   it's debuggable and the state is small; revisit only if snapshot size becomes
   a problem.

2. **`flat_config` duplication.** The snapshot contains `FlatConfig`, which the
   resuming process could instead reload from the original compiled artifact.
   Embedding it makes the snapshot self-contained (resume needs only the file);
   referencing it keeps snapshots tiny but couples resume to the artifact still
   existing and matching. Recommendation: embed — self-contained is worth the
   bytes, and it guarantees the resumed workflow runs the exact config it was
   suspended under.

3. **Resume value typing.** On the TypeScript side, `suspend(x)` must resolve to
   a typed value, but the resume value is supplied by the host out-of-band. This
   is the same `unknown`-on-the-JSON-path tradeoff `PRECOMPILATION.md` already
   accepts for `CompiledWorkflow.fromJSON`. The perform site can carry an output
   validator (like handlers do) so the resumed value is validated at the seam.

## Feasibility verdict

Feasible, and well-matched to the architecture. The engine is already a pure
data state machine with reified frames and existing effect-handler machinery,
so "stop and serialize everything" reduces to three concrete pieces of work:

1. Store the dispatched input value on the `Invoke` frame, alongside the
   `HandlerId` it already holds. Prerequisite for re-invoking in-flight handlers
   on resume; independently sensible (self-describing frame tree). Lands first as
   its own sub-refactor.
2. Make `WorkflowState` round-trip through bytes. The only obstacle is the
   generational `FrameId`; the fix is a serial-id translation on save/load via a
   parallel `WorkflowSnapshot` shape, leaving the runtime engine ungenericized.
   The irreducible core — it also unblocks the checkpoint-on-every-step model in
   `RESUMPTION.md`.
3. Add a `Suspend` effect that produces a `PendingEffectKind::Suspend`, handled
   by a new arm in `run_workflow` that snapshots and stops, with `run_workflow`
   returning a `Completed`/`Suspended` enum and a mirror `resume_workflow` that
   re-invokes every parked `Invoke` from `task_to_frame`, injects the resume
   value into the parked perform site, and re-enters the loop.

In-flight sibling handlers at suspend time are handled by killing them and
re-invoking on resume (requiring handler idempotency and the input-on-`Invoke`-
frame change above). Letting them drain to completion before snapshotting is a
deferred opt-in for workflows that can't tolerate double-runs.

This document is a feasibility assessment, not yet a task list. Next step is
splitting out the two prerequisite sub-refactors (input-on-frame,
arena-serialization) into their own docs, then a Phase-2 task breakdown for the
`Suspend` effect itself.
