/**
 * Event bus demo: filesystem-based producer/consumer queue with backpressure.
 *
 * 3 producers emit events at 100ms intervals. 1 consumer dequeues and processes.
 * Queue limit 5, max 10 events total. Runs in under 10s.
 *
 * Usage: pnpm exec tsx run.ts
 */

import {
  all,
  constant,
  loop,
  pipe,
  runPipeline,
  sleep,
  type TypedAction,
} from "@barnum/barnum/pipeline";
import { asOption } from "@barnum/barnum/pipeline";
import {
  clearQueue,
  consumeEvent,
  dequeueEvent,
  isDone,
  produceEvent,
} from "./handlers/steps";

// Producer: emit events at 100ms intervals until max reached.
// Returns None when max events produced (stopping the loop).
// Returns Some when backpressure hit (loop retries after short pause).
function makeProducerLoop(producerId: number): TypedAction<null, null> {
  return loop<null, null>((recur, done) =>
    pipe(sleep(100), constant({ producerId }), produceEvent).branch({
      Some: recur,
      None: done,
    }),
  );
}

// Consumer: dequeue and process events until all producers done and queue empty.
const consumerLoop: TypedAction<null, null> = loop<null, null>((recur, done) =>
  dequeueEvent.branch({
    Some: consumeEvent.then(recur),
    None: isDone.then(asOption()).branch({
      Some: done,
      None: sleep(50).then(recur),
    }),
  }),
);

// Clear queue, then run 3 producers + 1 consumer concurrently.
runPipeline(
  clearQueue.then(
    all(
      makeProducerLoop(0),
      makeProducerLoop(1),
      makeProducerLoop(2),
      consumerLoop,
    ),
  ),
);
