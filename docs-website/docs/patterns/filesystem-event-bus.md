# Filesystem Event Bus

When two concurrent branches need to communicate, a shared filesystem directory works as a message bus. The producer writes JSON files; the consumer reads and processes them. No engine extensions required — this uses `all`, `loop`, `branch`, and a reusable queue abstraction.

## Strategy 1: Delete on dequeue

The simplest approach. Consumer reads the oldest file and deletes it atomically. If the consumer crashes after dequeue but before finishing, the item is lost.

### Queue functions

```ts
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
  const files = readdirSync(dir).filter(f => f.endsWith(".json")).sort();
  if (files.length === 0) return none();

  const filepath = join(dir, files[0]);
  const raw = JSON.parse(readFileSync(filepath, "utf-8"));
  unlinkSync(filepath);
  return some(schema.parse(raw));
}
```

### Pipeline

```ts
all(
  // producer
  loop((recur, done) =>
    pipe(generateEvent, enqueueEvent).branch({
      Some: recur,
      None: done,
    }),
  ),
  // consumer
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

Use this when items are cheap to reproduce or losing an in-flight item is acceptable.

## Strategy 2: Claim and complete

Three states tracked via filename suffix:

| Suffix | Meaning |
|--------|---------|
| `.json` | Unclaimed — available for dequeue |
| `.pending.json` | Claimed — being processed |
| `.done.json` | Completed |

The consumer claims an item via `renameSync` (atomic on POSIX), processes it, then marks it complete with another rename. If the consumer crashes mid-processing, the `.pending.json` file survives and can be detected for recovery.

### Queue functions

```ts
export function enqueue<T>(dir: string, maxSize: number, item: T, schema: z.ZodType<T>): Option<null> {
  schema.parse(item);
  mkdirSync(dir, { recursive: true });
  const unclaimed = readdirSync(dir).filter(
    f => f.endsWith(".json") && !f.endsWith(".pending.json") && !f.endsWith(".done.json"),
  );
  if (unclaimed.length >= maxSize) {
    return none();
  }
  const id = `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  writeFileSync(join(dir, `${id}.json`), JSON.stringify(item));
  return some(null);
}

export function dequeue<T>(dir: string, schema: z.ZodType<T>): Option<{ id: string; item: T }> {
  mkdirSync(dir, { recursive: true });
  const files = readdirSync(dir)
    .filter(f => f.endsWith(".json") && !f.endsWith(".pending.json") && !f.endsWith(".done.json"))
    .sort();

  for (const file of files) {
    const id = file.replace(".json", "");
    const filepath = join(dir, file);
    const pendingPath = join(dir, `${id}.pending.json`);
    try {
      renameSync(filepath, pendingPath);
    } catch {
      continue; // another consumer claimed it first
    }
    const raw = JSON.parse(readFileSync(pendingPath, "utf-8"));
    return some({ id, item: schema.parse(raw) });
  }
  return none();
}

export function complete(dir: string, id: string): void {
  const pendingPath = join(dir, `${id}.pending.json`);
  const donePath = join(dir, `${id}.done.json`);
  renameSync(pendingPath, donePath);
}
```

### Typed handlers

```ts
export const dequeueEvent = createHandler({
  inputValidator: z.null(),
  outputValidator: optionSchema(ClaimedEventSchema),
  handle: async (): Promise<Option<{ id: string; item: Event }>> => {
    return dequeue(QUEUE_DIR, EventSchema);
  },
}, "dequeueEvent");

export const completeEvent = createHandler({
  inputValidator: z.object({ id: z.string() }),
  outputValidator: z.null(),
  handle: async ({ value: { id } }) => {
    complete(QUEUE_DIR, id);
    return null;
  },
}, "completeEvent");
```

### Pipeline

The consumer uses `bindInput` to thread the claimed event's `id` through processing and into the completion step:

```ts
all(
  makeProducerLoop(0),
  makeProducerLoop(1),
  makeProducerLoop(2),
  // consumer
  loop((recur) =>
    constant(null).then(dequeueEvent).branch({
      Some: bindInput<ClaimedEvent, never>((claimed) =>
        pipe(
          claimed.getField("item"),
          consumeEvent,
          claimed.pick("id"),
          completeEvent,
          recur,
        ),
      ),
      None: sleep(500).then(recur),
    }),
  ),
)
```

Use this when losing an in-flight item is unacceptable, when you need multiple concurrent consumers, or when you want an audit trail of completed items.

## Properties

Both strategies share:

- **Backpressure.** `enqueue` returns `None` when full. The pipeline branches on it.
- **FIFO.** Timestamp prefix on filenames. Lexicographic sort gives ordering.
- **Validation.** Both `enqueue` and `dequeue` validate against the Zod schema.
- **Debuggable.** `ls` the queue directory to see pending items.

Strategy 2 additionally provides:

- **Atomic claiming.** `renameSync` is atomic on POSIX. First consumer to rename wins; others get ENOENT and try the next file. Safe for multiple concurrent consumers.
- **Crash recovery.** `.pending.json` files indicate items that were claimed but never completed.
- **Audit trail.** `.done.json` files record what was processed.

## Limitations

- **Polling.** The consumer sleeps when the queue is empty. Latency equals the sleep interval.
- **Single machine.** The directory must be locally accessible to both branches.
- **Capacity race.** `readdirSync` count is best-effort. Concurrent producers may overshoot `maxSize` by up to (N-1).
- **Same-millisecond ordering.** Items enqueued in the same millisecond have arbitrary relative order.
