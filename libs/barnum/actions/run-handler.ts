const [handlerPath, exportName = "default"] = process.argv.slice(2);

const chunks: Buffer[] = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const envelope = JSON.parse(Buffer.concat(chunks).toString());

const mod = await import(handlerPath);
const handler = mod[exportName];

const results = await handler.handle(envelope);

process.stdout.write(JSON.stringify(results));
