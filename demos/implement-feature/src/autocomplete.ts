// Pre-existing autocomplete API client.
// This module is already part of the codebase — the feature task
// should USE it, not reimplement it.

const LIBRARIES = [
  "React",
  "Redux",
  "Router",
  "Relay",
  "Remix",
  "Next.js",
  "Nuxt",
  "Node",
  "Nest",
  "Nitro",
  "TypeScript",
  "Tailwind",
  "Turbo",
  "Three.js",
  "Tanstack",
  "Vite",
  "Vue",
  "Vitest",
  "Valibot",
  "Vanilla Extract",
];

export async function fetchSuggestions(query: string): Promise<string[]> {
  // Simulates network latency
  await new Promise((resolve) => setTimeout(resolve, 150));
  if (!query.trim()) return [];
  return LIBRARIES.filter((item) =>
    item.toLowerCase().includes(query.toLowerCase()),
  );
}
