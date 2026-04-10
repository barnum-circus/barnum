# Event loop pattern

## Context

The PR babysitting use case requires a long-running workflow that waits for external events (CI status changes, review comments, merge conflicts) and reacts to them. This is a general "event loop" pattern: poll for the next event, process it, repeat.

This doc explores how this maps to existing barnum primitives and whether new combinators or patterns are needed.

## The simple version

There's nothing algebraic about this. It's a `loop` that invokes a handler on each iteration. The handler blocks until an event is ready.

```typescript
// Concrete types for the PR babysitter
type PrEvent =
  | { kind: "CiCompleted"; value: { status: "passed" | "failed"; job: string } }
  | { kind: "ReviewSubmitted"; value: { state: "approved" | "changes_requested"; reviewer: string } }
  | { kind: "CommentAdded"; value: { author: string; body: string } }
  | { kind: "Closed"; value: {} };

const babysitPr: TypedAction<PrUrl, PrResult> = loop<PrResult, PrUrl>(
  (recur, done) =>
    pipe(
      waitForPrEvent,  // Invoke: blocks until the next event arrives
      branch({
        CiCompleted: pipe(handleCiResult, recur),
        ReviewSubmitted: pipe(handleReview, recur),
        CommentAdded: pipe(handleComment, recur),
        Closed: done,
      }),
    ),
);
```

`waitForPrEvent` is an ordinary Invoke handler. The runtime resolves it whenever an event is available — webhook, polling, whatever. The engine doesn't know or care.

This works today with no changes. The engine suspends at the Invoke, the runtime delivers an event via `complete()`, the loop body processes it, recurs, and the engine suspends again at the next `waitForPrEvent`.

## State across iterations

`loop` already handles this. The input to `recur` becomes the input of the next iteration:

```typescript
type LoopState = { pr_url: string; attempt_count: number; last_ci_status: string | null };

const babysitPr: TypedAction<PrUrl, PrResult> = pipe(
  // Initialize loop state
  augment(constant({ attempt_count: 0, last_ci_status: null })),

  loop<PrResult, LoopState>((recur, done) =>
    pipe(
      bindInput<LoopState>((state) =>
        pipe(
          state.getField("pr_url"),
          waitForPrEvent,
          branch({
            CiCompleted: pipe(
              // Update state with new CI status and recur
              augment(state),
              merge(),
              recur,
            ),
            Closed: pipe(drop, state, done),
            // ...
          }),
        ),
      ),
    ),
  ),
);
```

This is verbose but it composes. No new primitives needed for stateful event processing.

## Sinks: emitting actions during processing

The body can invoke handlers that produce side effects (post a comment, trigger a rebuild, merge the PR). These are just Invoke handlers in the pipeline:

```typescript
branch({
  CiCompleted: pipe(
    branch({
      passed: pipe(drop, postComment(constant("CI passed, merging.")), mergePr, done),
      failed: pipe(drop, postComment(constant("CI failed, investigating.")), triggerRebuild, recur),
    }),
  ),
  ReviewSubmitted: pipe(
    branch({
      approved: pipe(drop, postComment(constant("Thanks for the review!")), recur),
      changes_requested: pipe(respondToReviewFeedback, recur),
    }),
  ),
  // ...
})
```

Every "emit" is an Invoke. The engine dispatches it, the runtime executes it, the engine continues. Nothing new here.

## What would an effect-based version look like?

You could use Handle/Perform to decouple the event source from the consumer. The consumer Performs a "nextEvent" effect; an outer Handle intercepts it and dispatches to whatever event source is configured:

```typescript
function withEventSource<TEvent, TOut>(
  source: TypedAction<void, TEvent>,
  consumer: (nextEvent: TypedAction<void, TEvent>) => TypedAction<void, TOut>,
): TypedAction<void, TOut> {
  // nextEvent is a Perform. The Handle intercepts it, calls source,
  // and Resumes with the event.
  // ... builds Handle/Perform manually or via bind
}
```

This would let you swap the event source (webhooks vs polling vs test fixture) without changing the consumer. But it adds a layer of indirection for a dubious benefit — you can already swap the `waitForPrEvent` handler at the runtime level.

I don't think this is worth building. The simple Invoke-based version is clear and sufficient.

## What IS potentially useful: `forEach` over a stream

The current `forEach` takes an array and fans out. A stream variant would process elements one at a time as they arrive:

```typescript
// Hypothetical: forEachStream processes events sequentially as they arrive
const babysitPr = forEachStream<PrEvent, void>(
  waitForPrEvent,  // called repeatedly to get next event
  handleEvent,     // called once per event
);
```

This is just syntactic sugar over `loop` + `waitForPrEvent` + process. The question is whether it's common enough to justify a combinator.

For the PR babysitter specifically, the `loop` + `branch` version is more natural because different events have different control flow (some recur, some break). A generic `forEachStream` would need an escape hatch, which is just `earlyReturn` inside `loop` — what we already have.

## Conclusion

The event loop pattern maps directly to existing primitives:

- **`loop`** for the outer iteration
- **`branch`** for dispatching on event type
- **Invoke handlers** for waiting on events and emitting side effects
- **`recur`/`done`** for continue/break
- **`bindInput`** or `augment` for carrying state across iterations

No new combinators are needed. The PR babysitter workflow is just a `loop` that invokes a "wait for event" handler and branches on the result. The engine already supports this — the runtime just needs to resolve the handler when an event arrives.

The effect system (Handle/Perform) doesn't add value here because the "event source" isn't something you need to abstract over at the workflow level. The runtime already provides that abstraction.

## Open question: batching and debouncing

One thing the simple pattern doesn't handle well is event batching. If three CI status events arrive in quick succession, you might want to process only the latest. This is a runtime concern (the event source handler can debounce), not a workflow concern. But it's worth noting as a pattern that users will need guidance on.

Similarly: if the workflow is mid-processing when a new event arrives, the event is naturally queued — the engine won't call `waitForPrEvent` again until the current iteration completes and recurs. This is correct serial-processing behavior, but users should understand it.
