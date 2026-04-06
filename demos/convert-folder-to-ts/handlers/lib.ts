// Shared utilities for demo handlers.

import Anthropic from "@anthropic-ai/sdk";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
export const baseDir = path.resolve(__dirname, "..");

const client = new Anthropic();
const MODEL = "claude-sonnet-4-20250514";

/** Send a one-off prompt to Claude via the Anthropic SDK. Returns the text response. */
export async function callClaude(prompt: string): Promise<string> {
  console.error(`[callClaude] Sending prompt (${prompt.length} chars)...`);

  const response = await client.messages.create({
    model: MODEL,
    max_tokens: 8192,
    messages: [{ role: "user", content: prompt }],
  });

  let text = "";
  for (const block of response.content) {
    if (block.type === "text") {
      text += block.text;
    }
  }

  console.error(`[callClaude] Received response (${text.length} chars)`);
  return text;
}

/** Strip markdown code fences if present. */
export function stripCodeFences(text: string): string {
  const fenced = text.match(/^```(?:typescript|ts)?\n([\s\S]*?)\n```$/m);
  if (fenced) {
    return fenced[1];
  }
  return text.trim();
}
