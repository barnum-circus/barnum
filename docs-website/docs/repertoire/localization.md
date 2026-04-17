# Localization

Extract user-facing strings from a codebase, translate them into target languages, and verify the translations render correctly in context.

## Workflow

```ts
runPipeline(
  extractStrings
    .then(forEach(
      all(...targetLanguages.map(lang => translateTo(lang)))
        .then(forEach(verifyInContext)),
    ))
    .then(flattenResults)
    .then(writeLocaleFiles),
);
```

## Stages

1. **Extract strings** — scan the codebase for user-facing text. Output: array of `{ key, text, context }`.
2. **For each string** (concurrently):
   - **Translate** — `all` runs translations into every target language concurrently.
   - **Verify** — check each translation in context (e.g., does it fit the UI, is it culturally appropriate).
3. **Write locale files** — aggregate all translations and write them to the appropriate locale files.

## Key points

- Translation into different languages is embarrassingly parallel — `all` runs them concurrently.
- The verification agent sees the translation, the original text, and the UI context — but not other translations.
- The extraction step is deterministic (AST parsing or regex), not an LLM.
- `forEach` processes all strings concurrently, so the workflow scales with the number of strings.
