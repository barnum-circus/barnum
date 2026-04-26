# Repertoire

The Repertoire is a collection of real-world workflows that demonstrate what Barnum is for. Each entry describes a complete use case — the problem, the workflow structure, and the code.

For the underlying building blocks (loop, branch, race, etc.), see [Patterns](../patterns/index.md).

| Workflow | Description |
|---|---|
| [Adversarial review](./adversarial-review.md) | Implement → judge → revise loop until approved |
| [Identify and refactor](./identify-and-refactor.md) | Find refactoring opportunities, implement in worktrees, review with an LLM |
| [Code review](./code-review.md) | Parallel multi-check review (standards, security, performance) |
| [Document verification](./document-verification.md) | Extract claims from a document, verify each independently |
| [Legal review](./legal-review.md) | Parallel specialist analysis + synthesis |
| [Babysitting PRs](./babysitting-prs.md) | Monitor CI, respond to reviewers, auto-fix issues |
| [Codebase migration](./codebase-migration.md) | Convert an entire codebase file by file (JS→TS, class→hooks) |
| [Dependency updates](./dependency-updates.md) | Bump deps, fix breaking changes, verify tests |
| [Test generation](./test-generation.md) | Analyze code, generate tests, run them, iterate on failures |
| [Security remediation](./security-remediation.md) | Scan for vulnerabilities, classify, patch, verify |
| [Release management](./release-management.md) | Aggregate changelogs, bump versions, tag, publish |
| [Incident triage](./incident-triage.md) | Collect logs, correlate events, classify severity, draft runbook |
| [Onboarding automation](./onboarding-automation.md) | Analyze repo, generate setup guides, verify they work |
| [API contract verification](./api-contract-verification.md) | Diff API spec against implementation, flag breaking changes |
| [Localization](./localization.md) | Extract strings, translate, verify in context |
