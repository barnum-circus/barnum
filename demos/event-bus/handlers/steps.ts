import {
  mkdirSync,
  readdirSync,
  readFileSync,
  renameSync,
  unlinkSync,
  writeFileSync,
  existsSync,
} from "node:fs";
import { join } from "node:path";
import type { Option } from "@barnum/barnum/pipeline";
import {
  createHandler,
  none,
  optionSchema,
  some,
} from "@barnum/barnum/runtime";
import { z } from "zod";

// --- Config ---

const MAX_QUEUE_SIZE = 5;
const MAX_EVENTS = 10;
const QUEUE_DIR = join(import.meta.dirname, "../queue");
const COUNTER_FILE = join(QUEUE_DIR, ".produced_count");
const CONSUMER_MIN_MS = 50;
const CONSUMER_MAX_MS = 200;

mkdirSync(QUEUE_DIR, { recursive: true });

// --- Schemas ---

const EventSchema = z.object({
  id: z.string(),
  producerId: z.number(),
  timestamp: z.number(),
});

type Event = z.infer<typeof EventSchema>;

// --- Helpers ---

function randomBetween(min: number, max: number): number {
  return Math.floor(Math.random() * (max - min + 1)) + min;
}

function queueSize(): number {
  return readdirSync(QUEUE_DIR).filter((f) => f.endsWith(".unclaimed.json"))
    .length;
}

function producedCount(): number {
  if (!existsSync(COUNTER_FILE)) return 0;
  return parseInt(readFileSync(COUNTER_FILE, "utf-8"), 10) || 0;
}

function incrementProducedCount(): number {
  const count = producedCount() + 1;
  writeFileSync(COUNTER_FILE, String(count));
  return count;
}

function log(msg: string) {
  const ts = new Date().toLocaleTimeString();
  console.error(`[${ts}] ${msg}`);
}

// --- Handlers ---

export const clearQueue = createHandler(
  {
    inputValidator: z.null(),
    outputValidator: z.null(),
    handle: async () => {
      const files = readdirSync(QUEUE_DIR).filter(
        (f) => f.endsWith(".json") || f === ".produced_count",
      );
      for (const file of files) {
        unlinkSync(join(QUEUE_DIR, file));
      }
      log(`[setup] cleared queue`);
      return null;
    },
  },
  "clearQueue",
);

export const produceEvent = createHandler(
  {
    inputValidator: z.object({ producerId: z.number() }),
    outputValidator: optionSchema(z.null()),
    handle: async ({ value: { producerId } }): Promise<Option<null>> => {
      if (producedCount() >= MAX_EVENTS) {
        log(`[producer-${producerId}] max events reached, stopping`);
        return none();
      }

      const size = queueSize();
      if (size >= MAX_QUEUE_SIZE) {
        log(
          `[producer-${producerId}] backpressure: queue full (${size}/${MAX_QUEUE_SIZE})`,
        );
        return some(null);
      }

      const count = incrementProducedCount();
      const id = `${producerId}-${count}`;
      // Idempotent: skip if this ID already exists in any state
      const existing = readdirSync(QUEUE_DIR).find((f) => f.includes(id));
      if (existing) {
        return some(null);
      }
      const event = { id, producerId, timestamp: Date.now() };
      const filename = `${id}.unclaimed.json`;
      writeFileSync(join(QUEUE_DIR, filename), JSON.stringify(event));
      log(
        `[producer-${producerId}] produced event ${event.id} (queue: ${queueSize()}, total: ${count}/${MAX_EVENTS})`,
      );
      return some(null);
    },
  },
  "produceEvent",
);

export const dequeueEvent = createHandler(
  {
    inputValidator: z.null(),
    outputValidator: optionSchema(EventSchema),
    handle: async (): Promise<Option<Event>> => {
      const files = readdirSync(QUEUE_DIR)
        .filter((f) => f.endsWith(".unclaimed.json"))
        .sort();

      for (const file of files) {
        const filepath = join(QUEUE_DIR, file);
        const claimedPath = filepath.replace(
          ".unclaimed.json",
          ".claimed.json",
        );
        try {
          renameSync(filepath, claimedPath);
        } catch {
          continue;
        }
        const event = JSON.parse(readFileSync(claimedPath, "utf-8"));
        unlinkSync(claimedPath);
        log(
          `[dequeue] dequeued event ${event.id} (queue: ${queueSize()} remaining)`,
        );
        return some(event);
      }
      return none();
    },
  },
  "dequeueEvent",
);

export const consumeEvent = createHandler(
  {
    inputValidator: EventSchema,
    outputValidator: z.null(),
    handle: async ({ value: event }) => {
      const handleTime = randomBetween(CONSUMER_MIN_MS, CONSUMER_MAX_MS);
      log(
        `[consumer] handling event ${event.id} from producer-${event.producerId} (${handleTime}ms)`,
      );
      await new Promise((r) => setTimeout(r, handleTime));
      log(`[consumer] done handling event ${event.id}`);
      return null;
    },
  },
  "consumeEvent",
);

export const isDone = createHandler(
  {
    inputValidator: z.null(),
    outputValidator: z.boolean(),
    handle: async () => {
      return producedCount() >= MAX_EVENTS && queueSize() === 0;
    },
  },
  "isDone",
);
