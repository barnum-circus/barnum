## Done
- [x] readonly
- [x] update pending refactors, remove final --- section and update accordingly
- [x] test helper for asserting input and output types in one go (CheckIO)
- [x] sort inputs lint (eslint/sort-imports with ignoreDeclarationSort)
- [x] do not duplicate helpers — already centralized in tests/type-utils.ts
- [x] clean up toAction in postfix methods — use typedAction({ kind: "Chain", first: this, rest: ... }) directly

## TODO
- [ ] ast.ts remaining toAction uses in exported fns (forEach, branch, loop, etc.) — these are legitimate (erasing phantoms for Action-typed struct fields). May not need further cleanup.
- [ ] type tests need improvement: assertions should test input and output types are what we expect, var refs have appropriate types, and negative cases are tested
- [ ] all methods should accept var ref's, e.g. items.iterate().map(item => item.then(foo)). foo (and others) become functions accepting a var ref. items.iterate().map(foo) continues to work.
  - make sure to change all pipelines to functions accepting a var ref so the above is possible
  - THIS IS THE BIG ONE: requires overloads on map/flatMap/filter/andThen/etc. to accept both Pipeable and (varRef) => BodyResult forms