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
  return loop<null, null>((recur, done) => {
    const slept = sleep(100);
    const input = constant({ producerId }).call(slept);
    return produceEvent.call(input).branch({
      Some: recur,
      None: done,
    });
  });
}

// Consumer: dequeue, process, mark complete. Repeat until done.
function makeConsumerLoop(): TypedAction<null, null> {
  return loop<null, null>((recur, done) =>
    dequeueEvent.branch({
      Some: bindInput<ClaimedEvent, never>((claimed) => {
        const consumed = consumeEvent.call(claimed.getField("item"));
        const completed = completeEvent.call(claimed.pick("id")).call(consumed);
        return recur.call(completed);
      }),
      None: asOption()
        .call(isDone)
        .branch({
          Some: done,
          None: recur.call(sleep(50)),
        }),
    }),
  );
}

const concurrent = all(
  makeProducerLoop(0),
  makeProducerLoop(1),
  makeProducerLoop(2),
  makeConsumerLoop(),
  makeConsumerLoop(),
);
runPipeline(concurrent.call(clearQueue));
