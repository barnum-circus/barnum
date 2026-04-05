/**
 * Worker script: invoked as `tsx worker.ts <module> <export>`.
 *
 * Protocol:
 *   Rust → stdin:  JSON `{ "value": <any> }`
 *   stdout → Rust: JSON result (handler return value)
 *
 * If the handler throws or the module/export can't be resolved, the
 * process exits non-zero. Rust interprets that as a fatal workflow error.
 */

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

  // Write result to stdout
  process.stdout.write(JSON.stringify(result) ?? "null");
}

main().catch((error) => {
  process.stderr.write(`worker: ${error}\n`);
  process.exit(1);
});
