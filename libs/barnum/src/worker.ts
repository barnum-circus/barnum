/**
 * Worker script: invoked as `tsx worker.ts <module> <export>`.
 *
 * Protocol:
 *   Rust → stdin:  JSON `{ "value": <any> }`
 *   stdout → Rust: JSON result (handler return value)
 *
 * stdout is reserved for the protocol. All console output is redirected to
 * stderr so that handler code can freely use console.log for debugging.
 *
 * If the handler throws or the module/export can't be resolved, the
 * process exits non-zero. Rust interprets that as a fatal workflow error.
 */

// Redirect all console output to stderr — stdout is the protocol channel.
// This must happen before any handler code is imported or executed.
import { Console } from "node:console";

const stderrConsole = new Console({ stdout: process.stderr });
globalThis.console = stderrConsole;

// Suppress EPIPE — when the Rust binary exits (e.g., a race was resolved),
// orphan workers get broken pipe on stdout. This is expected, not an error.
process.stdout.on("error", (error: NodeJS.ErrnoException) => {
  if (error.code === "EPIPE") {
    process.exit(0);
  }
  throw error;
});

async function main(): Promise<void> {
  const [modulePath, exportName = "default"] = process.argv.slice(2);

  if (!modulePath) {
    process.stderr.write("worker: missing module path argument\n");
    process.exit(1);
  }

  // Read entire stdin
  const chunks: Buffer[] = [];
  for await (const chunk of process.stdin) {
    chunks.push(chunk);
  }
  const input = JSON.parse(Buffer.concat(chunks).toString());

  // Import handler, call it
  const mod = await import(modulePath);
  const handler = mod[exportName];

  if (!handler?.__definition?.handle) {
    process.stderr.write(
      `worker: ${modulePath}:${exportName} is not a barnum handler\n`,
    );
    process.exit(1);
  }

  const result = await handler.__definition.handle({ value: input.value });

  // Write result to stdout, then exit. Explicit exit is required because
  // importing the handler module may leave open handles (timers, servers,
  // etc.) that keep the Node event loop alive indefinitely.
  const json = JSON.stringify(result) ?? "null";
  process.stdout.write(json, () => {
    process.exit(0);
  });
}

main().catch((error) => {
  process.stderr.write(`worker: ${error}\n`);
  process.exit(1);
});
