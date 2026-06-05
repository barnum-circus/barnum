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
    pipe(sleep(100).drop(), produceEvent.call(constant({ producerId }))).branch(
      {
        Some: recur,
        None: done,
      },
    ),
  );
}

// Consumer: dequeue, process, mark complete. Repeat until done.
function makeConsumerLoop(): TypedAction<null, null> {
  return loop<null, null>((recur, done) =>
    dequeueEvent.branch({
      Some: bindInput<ClaimedEvent, never>((claimed) =>
        pipe(
          consumeEvent.call(claimed.getField("item")).drop(),
          recur.call(completeEvent.call(claimed.pick("id"))),
        ),
      ),
      None: asOption()
        .call(isDone)
        .branch({
          Some: done,
          None: recur.call(sleep(50)),
        }),
    }),
  );
}

pipe(
  clearQueue.drop(),
  all(
    makeProducerLoop(0),
    makeProducerLoop(1),
    makeProducerLoop(2),
    makeConsumerLoop(),
    makeConsumerLoop(),
  ),
).run();
