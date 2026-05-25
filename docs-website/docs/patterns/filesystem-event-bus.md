# Filesystem Event Bus

When two concurrent branches need to communicate, a shared filesystem directory works as a message bus. The producer writes JSON files; the consumer reads and deletes them. No engine extensions required — this uses `all`, `loop`, `branch`, and handlers you write yourself.

## Producer/consumer

```ts
all(
  // producer: generates work items, writes them to the queue directory
  loop((recur) =>
    pipe(
      checkQueueSize,
      branch({
        Full: sleep(60_000).then(recur),
        HasCapacity: pipe(generateWorkItem, writeToQueue, recur),
      }),
    ),
  ),
  // consumer: reads items from the queue, processes them
  loop((recur) =>
    pipe(
      readFromQueue,
      branch({
        Empty: sleep(60_000).then(recur),
        HasItem: pipe(processItem, recur),
      }),
    ),
  ),
)
```

Both loops run forever inside `all()`. The producer writes a JSON file to a shared directory for each work item. The consumer picks the oldest file, deletes it, and processes the contents.

## Backpressure

The producer checks how many files are in the queue directory. If the count exceeds a threshold, it sleeps and checks again. This prevents unbounded queue growth when the consumer is slower than the producer.

```ts
// checkQueueSize handler: counts files in the queue dir, returns { kind: "Full" } or { kind: "HasCapacity" }
// writeToQueue handler: serializes the item as JSON, writes to queue dir with a timestamp filename
// readFromQueue handler: reads + deletes the oldest file, returns { kind: "HasItem", value: T } or { kind: "Empty" }
```

## Why this works

- **Debuggable.** `ls` the queue directory to see pending items.
- **Crash-recoverable.** Files persist across restarts — unprocessed items survive a crash.
- **Ordered.** Timestamp-based filenames give FIFO ordering.
- **Zero coordination.** No locks needed — the producer creates new files, the consumer atomically reads and deletes existing ones.

## Limitations

- **Polling.** The consumer sleeps when the queue is empty. It doesn't wake instantly when a new item arrives — latency equals the sleep interval.
- **Single machine.** The directory must be locally accessible to both branches.
- **No type safety at the boundary.** The JSON serialization/deserialization is unvalidated unless your handlers include schema checks.

## How it works

Both branches are independent `loop` actions running concurrently via `all`. Each loop calls handlers (Invoke nodes) that interact with the filesystem. The engine has no knowledge of the queue — it just runs two concurrent loops that happen to share a directory. Coordination is entirely in the handler implementations.
