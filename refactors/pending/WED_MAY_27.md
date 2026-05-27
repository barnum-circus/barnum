- readonly
- update pending refactors, remove final --- section and update accordingly, move stuff to past, etc.
  - please update *this doc* with a summary of the changes, and things that I indicated could be worked on now

- sort inputs lint


types:
- do not duplicate helpers (e.g. IsExact), extract all into shared files
- test helper for asserting input and output types in one go.
- /Users/rbalicki/code/gsd/libs/barnum/src/ast.ts needs types everywhere
- clean up of toAction, etc and typedAction

e.g. function someMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(Option.some()));
}

does not need toAction!

- type tests need improvement: for type tests, the assertions should test that the input and output types are what we expect, and any var refs passed have the appropriate types, and that anything one might expect to pass but which doesn't (or vice versa) should be tested.

-----

- all methods, even those taking one param, should take var ref's, e.g. items.iterate().map(item => item.then(foo)), note that items.iterate().map(foo) will continue to work... foo (and others) will become functions accepting var ref's to pipelines, e.g. how I changed withResource
  - make sure to change all pipelines to functions accepting a var ref so the above is possible