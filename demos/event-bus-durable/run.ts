/**
 * Durable event bus demo: claim-and-complete strategy.
 *
 * Items transition through three states via filename suffix:
 *   .unclaimed.json → .pending.json → .done.json
 *
 * 3 producers, 2 consumers, queue limit 5, max 10 events.
 *
 * Usage: pnpm exec tsx run.ts
 */

import {
  all,
  bindInput,
  constant,
  loop,
  pipe,
  runPipeline,
  sleep,
  type TypedAction,
} from "@barnum/barnum/pipeline";
import { asOption } from "@barnum/barnum/pipeline";
import type { ClaimedEvent } from "./handlers/steps";
import {
  clearQueue,
  completeEvent,
  consumeEvent,
  dequeueEvent,
  isDone,
  produceEvent,
} from "./handlers/steps";

// Producer: emit events at 100ms intervals until max reached.
function makeProducerLoop(producerId: number): TypedAction<null, null> {
  return loop<null, null>((recur, done) =>
    pipe(sleep(100), constant({ producerId }), produceEvent).branch({
      Some: recur,
      None: done,
    }),
  );
}

// Consumer: dequeue, process, mark complete. Repeat until done.
function makeConsumerLoop(): TypedAction<null, null> {
  return loop<null, null>((recur, done) =>
    dequeueEvent.branch({
      Some: bindInput<ClaimedEvent, never>((claimed) =>
        pipe(
          claimed.getField("item"),
          consumeEvent,
          claimed.pick("id"),
          completeEvent,
          recur,
        ),
      ),
      None: isDone.then(asOption()).branch({
        Some: done,
        None: sleep(50).then(recur),
      }),
    }),
  );
}

runPipeline(
  clearQueue.then(
    all(
      makeProducerLoop(0),
      makeProducerLoop(1),
      makeProducerLoop(2),
      makeConsumerLoop(),
      makeConsumerLoop(),
    ),
  ),
);
