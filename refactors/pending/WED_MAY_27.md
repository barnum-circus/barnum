- /Users/rbalicki/code/gsd/libs/barnum/src/ast.ts needs types everywhere
- clean up of toAction, etc and typedAction

e.g. function someMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(Option.some()));
}

does not need toAction!

- all methods, even those taking one param, should take var ref's
- sort inputs lint
- test helper for asserting input and output types in one go.
- type tests need improvement: for type tests, the assertions should test that the input and output types are what we expect, and any var refs passed have the appropriate types, and that anything one might expect to pass but which doesn't (or vice versa) should be tested.