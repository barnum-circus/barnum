# Filesystem Event Bus

When two concurrent branches need to communicate, a shared filesystem directory works as a message bus. The producer writes JSON files; the consumer reads and deletes them. No engine extensions required — this uses `all`, `loop`, `branch`, and a reusable queue abstraction.

## The queue abstraction

A generic filesystem queue with typed enqueue/dequeue operations:

```ts
// shared/queue.ts

import { mkdirSync, readdirSync, readFileSync, renameSync, unlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { Option } from "@barnum/barnum/pipeline";
import { none, some } from "@barnum/barnum/runtime";
import type { z } from "zod";

export function enqueue<T>(dir: string, maxSize: number, item: T, schema: z.ZodType<T>): Option<null> {
  schema.parse(item);
  mkdirSync(dir, { recursive: true });
  const size = readdirSync(dir).filter(f => f.endsWith(".json")).length;
  if (size >= maxSize) {
    return none();
  }
  const id = `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  writeFileSync(join(dir, `${id}.json`), JSON.stringify(item));
  return some(null);
}

export function dequeue<T>(dir: string, schema: z.ZodType<T>): Option<T> {
  mkdirSync(dir, { recursive: true });
  const files = readdirSync(dir)
    .filter(f => f.endsWith(".json") && !f.endsWith(".claimed.json"))
    .sort();

  for (const file of files) {
    const filepath = join(dir, file);
    const claimedPath = filepath.replace(".json", ".claimed.json");
    try {
      renameSync(filepath, claimedPath);
    } catch {
      continue;
    }
    const raw = JSON.parse(readFileSync(claimedPath, "utf-8"));
    unlinkSync(claimedPath);
    return some(schema.parse(raw));
  }
  return none();
}

export function clearQueue(dir: string): void {
  mkdirSync(dir, { recursive: true });
  const files = readdirSync(dir).filter(f => f.endsWith(".json"));
  for (const file of files) {
    unlinkSync(join(dir, file));
  }
}
```

## Typed handlers

Each pipeline defines handlers that call the queue functions with a concrete schema and directory:

```ts
const QUEUE_DIR = join(import.meta.dirname, "../queue");
const MAX_SIZE = 5;

export const enqueueEvent = createHandler({
  inputValidator: EventSchema,
  outputValidator: optionSchema(z.null()),
  handle: async ({ value }): Promise<Option<null>> => {
    return enqueue(QUEUE_DIR, MAX_SIZE, value, EventSchema);
  },
}, "enqueueEvent");

export const dequeueEvent = createHandler({
  inputValidator: z.null(),
  outputValidator: optionSchema(EventSchema),
  handle: async (): Promise<Option<Event>> => {
    return dequeue(QUEUE_DIR, EventSchema);
  },
}, "dequeueEvent");
```

## Producer/consumer pipeline

```ts
all(
  // producer: generate events, enqueue with backpressure
  loop((recur, done) =>
    pipe(generateEvent, enqueueEvent).branch({
      Some: recur,   // enqueued or full (backpressure) — keep going
      None: done,    // done producing
    }),
  ),
  // consumer: dequeue and process until done
  loop((recur, done) =>
    dequeueEvent.branch({
      Some: processEvent.then(recur),
      None: isDone.then(asOption()).branch({
        Some: done,
        None: sleep(50).then(recur),
      }),
    }),
  ),
)
```

Both loops run concurrently inside `all()`. The producer enqueues items; `enqueue` returns `None` when the queue is full (backpressure). The consumer dequeues and processes until the queue is empty and all producers are done.

## Properties

- **Backpressure.** `enqueue` returns `None` when full. The pipeline branches on it — sleep and retry, or stop.
- **Atomic claiming.** `renameSync` is atomic on POSIX. First consumer to rename wins; others get ENOENT and try the next file. Safe for multiple concurrent consumers.
- **FIFO.** Timestamp prefix on filenames. Lexicographic sort gives ordering.
- **Validation.** Both `enqueue` and `dequeue` validate against the Zod schema. Bad data never hits disk; corrupted files fail at dequeue.
- **Debuggable.** `ls` the queue directory to see pending items. `cat` any file to inspect.
- **Crash-recoverable.** Files persist across restarts — unprocessed items survive a crash.

## Limitations

- **Polling.** The consumer sleeps when the queue is empty. Latency equals the sleep interval.
- **Single machine.** The directory must be locally accessible to both branches.
- **Capacity race.** `readdirSync` count is best-effort. Concurrent producers may overshoot `maxSize` by up to (N-1).
- **Same-millisecond ordering.** Items enqueued in the same millisecond have arbitrary relative order.
