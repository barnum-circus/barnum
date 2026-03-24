// Extracts the config JSON from a barnum.config.ts file by intercepting
// BarnumConfig.fromConfig. Used by scripts/generate-graphs.sh.
//
// Usage: tsx extract-config.ts <path-to-barnum.config.ts>

import { BarnumConfig } from "@barnum/barnum";

const configPath = process.argv[2];
if (!configPath) {
  process.stderr.write("Usage: tsx extract-config.ts <config-file>\n");
  process.exit(1);
}

let captured: unknown;
BarnumConfig.fromConfig = ((config: unknown) => {
  captured = config;
  return { run: () => ({ on: () => {} }) };
}) as typeof BarnumConfig.fromConfig;

await import(configPath);

if (captured === undefined) {
  process.stderr.write("ERROR: BarnumConfig.fromConfig was never called\n");
  process.exit(1);
}

process.stdout.write(JSON.stringify(captured));
