import { describe, it, expect, beforeEach } from "vitest";
import {
  all,
  config,
  loop,
  branch,
  pipe,
  forEach,
  bind,
  bindInput,
  resetEffectIdCounter,
  type OptionDef,
} from "../src/ast.js";
import {
  constant,
  done,
  drop,
  identity,
  merge,
  recur,
  tag,
  Option as O,
  Result as R,
} from "../src/builtins.js";

import {
  setup,
  build,
  verify,
  deploy,
  healthCheck,
  listFiles,
  migrate,
  typeCheck,
  classifyErrors,
  fix,
} from "./handlers.js";

// -----------------------------------------------------------------------
// Pipe
// -----------------------------------------------------------------------

describe("pipe", () => {
  it("chains setup → build → verify → deploy", () => {
    const cfg = config(
      pipe(
        constant({ project: "test" }),
        setup,
        build,
        verify,
        deploy,
      ),
    );
    expect(cfg.workflow.kind).toBe("Chain");
  });

  it("rejects mismatched types", () => {
    // verify outputs { verified: boolean }, setup expects { project: string }
    // @ts-expect-error — type mismatch between verify's output and setup's input
    pipe(verify, setup);
  });

  it("chains three steps correctly", () => {
    const workflow = pipe(setup, build, verify);
    expect(workflow.kind).toBe("Chain");
  });

  it("rejects unrelated types", () => {
    // deploy outputs { deployed: true }, setup expects { project: string }
    // @ts-expect-error — type mismatch between deploy's output and setup's input
    pipe(deploy, setup);
  });
});

// -----------------------------------------------------------------------
// ForEach
// -----------------------------------------------------------------------

describe("forEach", () => {
  it("maps input/output to arrays", () => {
    const workflow = forEach(verify);
    expect(workflow.kind).toBe("ForEach");
  });

  it("composes with pipe: setup → listFiles → forEach(migrate)", () => {
    const cfg = config(
      pipe(
        constant({ project: "test" }),
        setup,
        listFiles,
        forEach(migrate),
      ),
    );
    expect(cfg.workflow.kind).toBe("Chain");
  });
});

// -----------------------------------------------------------------------
// All
// -----------------------------------------------------------------------

describe("all", () => {
  it("accepts actions with the same input type", () => {
    const workflow = all(verify, verify);
    expect(workflow.kind).toBe("All");
  });

  it("rejects actions with different input types", () => {
    // setup expects { project: string }, verify expects { artifact: string }
    // @ts-expect-error — input types do not unify
    all(setup, verify);
  });

  it("composes with all and branch", () => {
    const cfg = config(
      pipe(
        constant({ project: "test" }),
        all(
          setup,
          pipe(
            setup,
            build,
          ),
        ),
      ),
    );
    expect(cfg.workflow.kind).toBe("Chain");
  });
});

// -----------------------------------------------------------------------
// Branch
// -----------------------------------------------------------------------

describe("branch", () => {
  it("accepts cases with the same output type", () => {
    const workflow = branch({
      Yes: deploy,
      No: deploy,
    });
    expect(workflow.kind).toBe("Branch");
  });

  it("rejects output flowing into incompatible step", () => {
    // branch outputs { deployed: true }, but setup expects { project: string }
    // @ts-expect-error — branch output doesn't satisfy next step's input
    pipe(branch({ A: deploy, B: deploy }), setup);
  });
});

// -----------------------------------------------------------------------
// Loop
// -----------------------------------------------------------------------

describe("loop", () => {
  it("accepts body returning LoopResult", () => {
    const workflow = loop(healthCheck);
    expect(workflow.kind).toBe("Loop");
  });

  it("rejects body not returning LoopResult", () => {
    // verify: { artifact: string } → { verified: boolean } — not a LoopResult
    // @ts-expect-error — loop body must return LoopResult<In, Out>
    loop(verify);
  });

  it("composes type-check loop with branch", () => {
    const cfg = config(
      pipe(
        constant({ project: "test" }),
        setup,
        listFiles,
        forEach(migrate),
        loop(
          pipe(
            drop<any>(),
            typeCheck,
            classifyErrors,
            branch({
              HasErrors: pipe(forEach(fix), recur<any, void>()),
              Clean: done<any, void>(),
            }),
          ),
        ),
      ),
    );
    expect(cfg.workflow.kind).toBe("Chain");
  });
});

// -----------------------------------------------------------------------
// Postfix operators
// -----------------------------------------------------------------------

describe("postfix operators", () => {
  it(".branch() produces Chain → Branch AST", () => {
    const action = classifyErrors.branch({
      HasErrors: drop(),
      Clean: drop(),
    });
    expect(action.kind).toBe("Chain");
    const chain = action as { kind: "Chain"; first: any; rest: any };
    expect(chain.first.kind).toBe("Invoke");
    expect(chain.rest.kind).toBe("Branch");
    expect(Object.keys(chain.rest.cases)).toEqual(["HasErrors", "Clean"]);
  });

  it(".flatten() produces Chain → Flatten AST", () => {
    const action = forEach(forEach(verify)).flatten();
    expect(action.kind).toBe("Chain");
    const chain = action as { kind: "Chain"; first: any; rest: any };
    expect(chain.first.kind).toBe("ForEach");
    expect(chain.rest.kind).toBe("Invoke");
    expect(chain.rest.handler.builtin.kind).toBe("Flatten");
  });

  it(".drop() produces Chain → Drop AST", () => {
    const action = setup.drop();
    expect(action.kind).toBe("Chain");
    const chain = action as { kind: "Chain"; first: any; rest: any };
    expect(chain.first.kind).toBe("Invoke");
    expect(chain.rest.kind).toBe("Invoke");
    expect(chain.rest.handler.builtin.kind).toBe("Drop");
  });

  it(".tag() produces Chain → Tag AST", () => {
    const action = verify.tag<{ Ok: { verified: boolean } }, "Ok">("Ok");
    expect(action.kind).toBe("Chain");
    const chain = action as { kind: "Chain"; first: any; rest: any };
    expect(chain.first.kind).toBe("Invoke");
    expect(chain.rest.kind).toBe("Invoke");
    expect(chain.rest.handler.builtin.kind).toBe("Tag");
    expect(chain.rest.handler.builtin.value).toBe("Ok");
  });

  it(".get() produces Chain → ExtractField AST", () => {
    const action = setup.get("project");
    expect(action.kind).toBe("Chain");
    const chain = action as { kind: "Chain"; first: any; rest: any };
    expect(chain.first.kind).toBe("Invoke");
    expect(chain.rest.kind).toBe("Invoke");
    expect(chain.rest.handler.builtin.kind).toBe("ExtractField");
    expect(chain.rest.handler.builtin.value).toBe("project");
  });

  it("postfix methods are chainable", () => {
    // forEach(analyze).flatten().forEach().drop()
    const action = forEach(verify).flatten().drop();
    expect(action.kind).toBe("Chain");
  });

  it("postfix .branch() produces valid AST for loop pattern", () => {
    // Equivalent to: pipe(typeCheck, classifyErrors, branch({ ... }))
    // Chain nesting differs (left vs right associative) but semantically equivalent
    const action = pipe(typeCheck, classifyErrors).branch({
      HasErrors: forEach(fix),
      Clean: drop(),
    });
    expect(action.kind).toBe("Chain");
    // The rest should be a Branch node
    const chain = action as { kind: "Chain"; first: any; rest: any };
    expect(chain.rest.kind).toBe("Branch");
    expect(Object.keys(chain.rest.cases)).toEqual(["HasErrors", "Clean"]);
  });
});

// -----------------------------------------------------------------------
// Option namespace
// -----------------------------------------------------------------------

describe("Option namespace", () => {
  it("Option.some() produces Tag('Some') AST", () => {
    const action = O.some<string>();
    expect(action.kind).toBe("Invoke");
    const invoke = action as { kind: "Invoke"; handler: any };
    expect(invoke.handler.builtin.kind).toBe("Tag");
    expect(invoke.handler.builtin.value).toBe("Some");
  });

  it("Option.none() produces Tag('None') AST", () => {
    const action = O.none<string>();
    expect(action.kind).toBe("Invoke");
    const invoke = action as { kind: "Invoke"; handler: any };
    expect(invoke.handler.builtin.kind).toBe("Tag");
    expect(invoke.handler.builtin.value).toBe("None");
  });

  it("Option.map() produces Branch with Some and None cases", () => {
    const action = O.map(verify);
    expect(action.kind).toBe("Branch");
    const branch = action as { kind: "Branch"; cases: any };
    expect(Object.keys(branch.cases).sort()).toEqual(["None", "Some"]);
    // Some case: ExtractField("value") → Chain(verify, Tag("Some"))
    const someCase = branch.cases["Some"];
    expect(someCase.kind).toBe("Chain");
    expect(someCase.first.handler.builtin.kind).toBe("ExtractField");
    expect(someCase.rest.kind).toBe("Chain");
    // None case: ExtractField("value") → Tag("None")
    const noneCase = branch.cases["None"];
    expect(noneCase.kind).toBe("Chain");
    expect(noneCase.rest.handler.builtin.kind).toBe("Tag");
    expect(noneCase.rest.handler.builtin.value).toBe("None");
  });

  it("Option.andThen() produces Branch with action Some and Tag None", () => {
    const action = O.andThen(pipe(verify, O.some<{ verified: boolean }>()));
    expect(action.kind).toBe("Branch");
    const branch = action as { kind: "Branch"; cases: any };
    expect(Object.keys(branch.cases).sort()).toEqual(["None", "Some"]);
    // Some case: ExtractField("value") → Chain(verify, Tag("Some"))
    const someCase = branch.cases["Some"];
    expect(someCase.kind).toBe("Chain");
    expect(someCase.first.handler.builtin.kind).toBe("ExtractField");
    // None case: ExtractField("value") → Tag("None")
    const noneCase = branch.cases["None"];
    expect(noneCase.rest.handler.builtin.kind).toBe("Tag");
    expect(noneCase.rest.handler.builtin.value).toBe("None");
  });

  it("Option.unwrapOr() produces Branch with identity Some and drop+default None", () => {
    const action = O.unwrapOr(constant("fallback"));
    expect(action.kind).toBe("Branch");
    const branch = action as { kind: "Branch"; cases: any };
    // Some case: ExtractField("value") → Identity
    const someCase = branch.cases["Some"];
    expect(someCase.rest.handler.builtin.kind).toBe("Identity");
    // None case: ExtractField("value") → Chain(Drop, Constant("fallback"))
    const noneCase = branch.cases["None"];
    expect(noneCase.rest.kind).toBe("Chain");
    expect(noneCase.rest.first.handler.builtin.kind).toBe("Drop");
    expect(noneCase.rest.rest.handler.builtin.kind).toBe("Constant");
    expect(noneCase.rest.rest.handler.builtin.value).toBe("fallback");
  });

  it("Option.flatten() produces Branch with identity Some and Tag None", () => {
    const action = O.flatten<string>();
    expect(action.kind).toBe("Branch");
    const branch = action as { kind: "Branch"; cases: any };
    expect(branch.cases["Some"].rest.handler.builtin.kind).toBe("Identity");
    expect(branch.cases["None"].rest.handler.builtin.kind).toBe("Tag");
    expect(branch.cases["None"].rest.handler.builtin.value).toBe("None");
  });

  it("Option.filter() produces Branch with predicate Some and Tag None", () => {
    const predicate = O.some<string>();
    const action = O.filter(predicate);
    expect(action.kind).toBe("Branch");
    const branch = action as { kind: "Branch"; cases: any };
    // Some case body is the predicate (Tag "Some")
    expect(branch.cases["Some"].rest.handler.builtin.kind).toBe("Tag");
    expect(branch.cases["Some"].rest.handler.builtin.value).toBe("Some");
  });

  it("Option.collect() produces CollectSome builtin", () => {
    const action = O.collect<string>();
    expect(action.kind).toBe("Invoke");
    const invoke = action as { kind: "Invoke"; handler: any };
    expect(invoke.handler.builtin.kind).toBe("CollectSome");
  });

  it("Option.isSome() produces Branch with Constant(true) and Constant(false)", () => {
    const action = O.isSome<string>();
    expect(action.kind).toBe("Branch");
    const branch = action as { kind: "Branch"; cases: any };
    // Some → Drop → Constant(true)
    const someChain = branch.cases["Some"].rest;
    expect(someChain.kind).toBe("Chain");
    expect(someChain.rest.handler.builtin.kind).toBe("Constant");
    expect(someChain.rest.handler.builtin.value).toBe(true);
    // None → Drop → Constant(false)
    const noneChain = branch.cases["None"].rest;
    expect(noneChain.kind).toBe("Chain");
    expect(noneChain.rest.handler.builtin.kind).toBe("Constant");
    expect(noneChain.rest.handler.builtin.value).toBe(false);
  });

  it("Option.isNone() is the inverse of isSome", () => {
    const action = O.isNone<string>();
    expect(action.kind).toBe("Branch");
    const branch = action as { kind: "Branch"; cases: any };
    // Some → Drop → Constant(false)
    expect(branch.cases["Some"].rest.rest.handler.builtin.value).toBe(false);
    // None → Drop → Constant(true)
    expect(branch.cases["None"].rest.rest.handler.builtin.value).toBe(true);
  });

  it("postfix .mapOption() produces Chain → Branch AST", () => {
    const optionAction = verify.tag<OptionDef<{ verified: boolean }>, "Some">("Some");
    const mapped = optionAction.mapOption(deploy);
    expect(mapped.kind).toBe("Chain");
    const chain = mapped as { kind: "Chain"; first: any; rest: any };
    expect(chain.first.kind).toBe("Chain");
    expect(chain.rest.kind).toBe("Branch");
    expect(Object.keys(chain.rest.cases).sort()).toEqual(["None", "Some"]);
  });
});

// -----------------------------------------------------------------------
// Reader monad pattern
//
// all(identity(), handler) → merge()
// Preserves the original input alongside the handler's output.
// -----------------------------------------------------------------------

describe("reader monad pattern", () => {
  it("preserves context via all + identity + merge", () => {
    const cfg = config(
      pipe(
        constant({ initialized: true, project: "test" }),
        all(identity<{ initialized: boolean; project: string }>(), build),
        merge<[{ initialized: boolean; project: string }, { artifact: string }]>(),
      ),
    );
    expect(cfg.workflow.kind).toBe("Chain");
  });
});

// -----------------------------------------------------------------------
// bind / bindInput
// -----------------------------------------------------------------------

describe("bind", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("single binding produces Chain(All(..., Identity), Handle(readVar, Chain(ExtractIndex, body)))", () => {
    const exprA = constant(42);
    const bodyAction = identity<number>();
    const result = bind([exprA], ([_a]) => bodyAction);

    // Outer: Chain
    expect(result.kind).toBe("Chain");
    const outer = result as { kind: "Chain"; first: any; rest: any };

    // First: All with 2 actions (binding + Identity)
    expect(outer.first.kind).toBe("All");
    expect(outer.first.actions).toHaveLength(2);
    expect(outer.first.actions[0]).toEqual(exprA);
    expect(outer.first.actions[1].handler.builtin.kind).toBe("Identity");

    // Rest: Handle
    expect(outer.rest.kind).toBe("Handle");
    const handle = outer.rest as { kind: "Handle"; effect_id: number; handler: any; body: any };
    expect(typeof handle.effect_id).toBe("number");

    // Handle handler: readVar(0) = Chain(ExtractField("state"), Chain(ExtractIndex(0), Tag("Resume")))
    expect(handle.handler.kind).toBe("Chain");
    expect(handle.handler.first.handler.builtin.kind).toBe("ExtractField");
    expect(handle.handler.first.handler.builtin.value).toBe("state");
    expect(handle.handler.rest.first.handler.builtin.kind).toBe("ExtractIndex");
    expect(handle.handler.rest.first.handler.builtin.value).toBe(0);
    expect(handle.handler.rest.rest.handler.builtin.kind).toBe("Tag");
    expect(handle.handler.rest.rest.handler.builtin.value).toBe("Resume");

    // Handle body: Chain(ExtractIndex(1), bodyAction)
    expect(handle.body.kind).toBe("Chain");
    expect(handle.body.first.handler.builtin.kind).toBe("ExtractIndex");
    expect(handle.body.first.handler.builtin.value).toBe(1);
  });

  it("two bindings produce two nested Handles with distinct effectIds", () => {
    const exprA = constant("alice");
    const exprB = constant(99);
    const bodyAction = identity<string>();
    const result = bind([exprA, exprB], ([_a, _b]) => bodyAction);

    const outer = result as { kind: "Chain"; first: any; rest: any };

    // All with 3 actions (2 bindings + Identity)
    expect(outer.first.kind).toBe("All");
    expect(outer.first.actions).toHaveLength(3);

    // Outer Handle
    const handle0 = outer.rest;
    expect(handle0.kind).toBe("Handle");

    // Inner Handle
    const handle1 = handle0.body;
    expect(handle1.kind).toBe("Handle");

    // Distinct effectIds
    expect(handle0.effect_id).not.toBe(handle1.effect_id);

    // readVar indices: outer=0, inner=1
    expect(handle0.handler.rest.first.handler.builtin.value).toBe(0);
    expect(handle1.handler.rest.first.handler.builtin.value).toBe(1);

    // Innermost body: Chain(ExtractIndex(2), bodyAction) — pipeline_input at index 2
    expect(handle1.body.kind).toBe("Chain");
    expect(handle1.body.first.handler.builtin.kind).toBe("ExtractIndex");
    expect(handle1.body.first.handler.builtin.value).toBe(2);
  });

  it("VarRef is a Perform node with unique effectId", () => {
    const exprA = constant("x");
    let capturedVarRef: any;
    bind([exprA], ([a]) => {
      capturedVarRef = a;
      return identity();
    });

    expect(capturedVarRef.kind).toBe("Perform");
    expect(typeof capturedVarRef.effect_id).toBe("number");
  });

  it("effectIds are unique across separate bind calls", () => {
    const effectIds: number[] = [];
    bind([constant(1), constant(2)], ([_a, _b]) => {
      return identity();
    });
    // First bind uses effectIds 0, 1

    let ref1: any, ref2: any;
    bind([constant(3), constant(4)], ([a, b]) => {
      ref1 = a;
      ref2 = b;
      return identity();
    });
    // Second bind uses effectIds 2, 3
    effectIds.push(ref1.effect_id, ref2.effect_id);

    // All four effectIds (0, 1, 2, 3) are distinct
    expect(ref1.effect_id).not.toBe(0);
    expect(ref1.effect_id).not.toBe(1);
    expect(ref2.effect_id).not.toBe(0);
    expect(ref2.effect_id).not.toBe(1);
    expect(ref1.effect_id).not.toBe(ref2.effect_id);
  });

  it("readVar(n) structure is Chain(ExtractField('state'), Chain(ExtractIndex(n), Tag('Resume')))", () => {
    // Verify readVar structure for n=0, n=1, n=2 by inspecting handles in a 3-binding bind
    const result = bind(
      [constant("a"), constant("b"), constant("c")],
      ([_a, _b, _c]) => identity(),
    );

    const outer = result as { kind: "Chain"; first: any; rest: any };
    const handle0 = outer.rest;
    const handle1 = handle0.body;
    const handle2 = handle1.body;

    for (const [handle, expectedIndex] of [[handle0, 0], [handle1, 1], [handle2, 2]] as const) {
      const handler = handle.handler;
      expect(handler.kind).toBe("Chain");
      // ExtractField("state")
      expect(handler.first.handler.builtin.kind).toBe("ExtractField");
      expect(handler.first.handler.builtin.value).toBe("state");
      // ExtractIndex(n)
      expect(handler.rest.first.handler.builtin.kind).toBe("ExtractIndex");
      expect(handler.rest.first.handler.builtin.value).toBe(expectedIndex);
      // Tag("Resume")
      expect(handler.rest.rest.handler.builtin.kind).toBe("Tag");
      expect(handler.rest.rest.handler.builtin.value).toBe("Resume");
    }
  });
});

describe("bindInput", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("compiles to bind([identity()], ([input]) => pipe(drop(), body(input)))", () => {
    const bodyAction = constant("result");
    const result = bindInput<string, string>((_input) => bodyAction);

    // Outer: Chain(All(Identity, Identity), Handle(...))
    const outer = result as { kind: "Chain"; first: any; rest: any };
    expect(outer.first.kind).toBe("All");
    expect(outer.first.actions).toHaveLength(2);
    // First action is identity (from bind([identity()], ...))
    expect(outer.first.actions[0].handler.builtin.kind).toBe("Identity");
    // Second action is identity (pipeline input preservation)
    expect(outer.first.actions[1].handler.builtin.kind).toBe("Identity");

    // Handle wraps the body
    expect(outer.rest.kind).toBe("Handle");
    const handle = outer.rest;

    // Handle body: Chain(ExtractIndex(1), Chain(Drop, bodyAction))
    expect(handle.body.kind).toBe("Chain");
    expect(handle.body.first.handler.builtin.kind).toBe("ExtractIndex");
    expect(handle.body.first.handler.builtin.value).toBe(1);

    // The rest of the body is Chain(Drop, bodyAction)
    const bodyChain = handle.body.rest;
    expect(bodyChain.kind).toBe("Chain");
    expect(bodyChain.first.handler.builtin.kind).toBe("Drop");
  });

  it("VarRef from bindInput is a Perform node", () => {
    let capturedRef: any;
    bindInput<string, string>((input) => {
      capturedRef = input;
      return constant("result");
    });

    expect(capturedRef.kind).toBe("Perform");
    expect(typeof capturedRef.effect_id).toBe("number");
  });
});

// ---------------------------------------------------------------------------
// Result namespace — AST structure tests
// ---------------------------------------------------------------------------

describe("Result combinators", () => {
  it("Result.ok() produces Tag('Ok')", () => {
    const action = R.ok();
    expect(action).toEqual({
      kind: "Invoke",
      handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Ok" } },
    });
  });

  it("Result.err() produces Tag('Err')", () => {
    const action = R.err();
    expect(action).toEqual({
      kind: "Invoke",
      handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Err" } },
    });
  });

  it("Result.map(action) desugars to Branch(Ok: Chain(action, Tag(Ok)), Err: Tag(Err))", () => {
    const action = R.map(setup);
    expect(action).toEqual({
      kind: "Branch",
      cases: {
        Ok: {
          kind: "Chain",
          first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "ExtractField", value: "value" } } },
          rest: { kind: "Chain", first: setup, rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Ok" } } } },
        },
        Err: {
          kind: "Chain",
          first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "ExtractField", value: "value" } } },
          rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Err" } } },
        },
      },
    });
  });

  it("Result.mapErr(action) desugars to Branch(Ok: Tag(Ok), Err: Chain(action, Tag(Err)))", () => {
    const action = R.mapErr(setup);
    expect(action).toEqual({
      kind: "Branch",
      cases: {
        Ok: {
          kind: "Chain",
          first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "ExtractField", value: "value" } } },
          rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Ok" } } },
        },
        Err: {
          kind: "Chain",
          first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "ExtractField", value: "value" } } },
          rest: { kind: "Chain", first: setup, rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Err" } } } },
        },
      },
    });
  });

  it("Result.andThen(action) desugars to Branch(Ok: action, Err: Tag(Err))", () => {
    const inner = R.ok<string, string>();
    const result = R.andThen(inner);
    const branch = result as any;
    expect(branch.kind).toBe("Branch");
    // Ok case: ExtractValue → action
    expect(branch.cases.Ok.rest).toBe(inner);
    // Err case: ExtractValue → Tag(Err)
    expect(branch.cases.Err.rest.handler.builtin.kind).toBe("Tag");
    expect(branch.cases.Err.rest.handler.builtin.value).toBe("Err");
  });

  it("Result.or(fallback) desugars to Branch(Ok: Tag(Ok), Err: fallback)", () => {
    const fallback = R.ok<string, string>();
    const result = R.or(fallback);
    const branch = result as any;
    expect(branch.kind).toBe("Branch");
    expect(branch.cases.Ok.rest.handler.builtin.value).toBe("Ok");
    expect(branch.cases.Err.rest).toBe(fallback);
  });

  it("Result.and(other) desugars to Branch(Ok: Chain(Drop, other), Err: Tag(Err))", () => {
    const other = pipe(constant("replacement"), R.ok<string, string>());
    const result = R.and(other);
    const branch = result as any;
    expect(branch.kind).toBe("Branch");
    // Ok case: ExtractValue → Chain(Drop, other)
    const okBody = branch.cases.Ok.rest;
    expect(okBody.kind).toBe("Chain");
    expect(okBody.first.handler.builtin.kind).toBe("Drop");
    expect(okBody.rest).toBe(other);
  });

  it("Result.unwrapOr(default) desugars to Branch(Ok: Identity, Err: default)", () => {
    const fallback = constant("default");
    const action = R.unwrapOr(fallback);
    const branch = action as any;
    expect(branch.kind).toBe("Branch");
    expect(branch.cases.Ok.rest.handler.builtin.kind).toBe("Identity");
    expect(branch.cases.Err.rest).toBe(fallback);
  });

  it("Result.flatten() desugars to Branch(Ok: Identity, Err: Tag(Err))", () => {
    const action = R.flatten();
    const branch = action as any;
    expect(branch.kind).toBe("Branch");
    expect(branch.cases.Ok.rest.handler.builtin.kind).toBe("Identity");
    expect(branch.cases.Err.rest.handler.builtin.value).toBe("Err");
  });

  it("Result.toOption() desugars to Branch(Ok: Tag(Some), Err: Chain(Drop, Tag(None)))", () => {
    const action = R.toOption();
    const branch = action as any;
    expect(branch.kind).toBe("Branch");
    expect(branch.cases.Ok.rest.handler.builtin.value).toBe("Some");
    expect(branch.cases.Err.rest.first.handler.builtin.kind).toBe("Drop");
    expect(branch.cases.Err.rest.rest.handler.builtin.value).toBe("None");
  });

  it("Result.toOptionErr() desugars to Branch(Ok: Chain(Drop, Tag(None)), Err: Tag(Some))", () => {
    const action = R.toOptionErr();
    const branch = action as any;
    expect(branch.kind).toBe("Branch");
    expect(branch.cases.Ok.rest.first.handler.builtin.kind).toBe("Drop");
    expect(branch.cases.Ok.rest.rest.handler.builtin.value).toBe("None");
    expect(branch.cases.Err.rest.handler.builtin.value).toBe("Some");
  });

  it("Result.isOk() desugars to Branch(Ok: Constant(true), Err: Constant(false))", () => {
    const action = R.isOk();
    const branch = action as any;
    expect(branch.kind).toBe("Branch");
    expect(branch.cases.Ok.rest.rest.handler.builtin.value).toBe(true);
    expect(branch.cases.Err.rest.rest.handler.builtin.value).toBe(false);
  });

  it("Result.isErr() desugars to Branch(Ok: Constant(false), Err: Constant(true))", () => {
    const action = R.isErr();
    const branch = action as any;
    expect(branch.kind).toBe("Branch");
    expect(branch.cases.Ok.rest.rest.handler.builtin.value).toBe(false);
    expect(branch.cases.Err.rest.rest.handler.builtin.value).toBe(true);
  });

  it("Result.transpose() desugars to nested branches", () => {
    const action = R.transpose();
    const branch = action as any;
    expect(branch.kind).toBe("Branch");
    // Ok case: receives Option, branches on Some/None
    const okBody = branch.cases.Ok.rest;
    expect(okBody.kind).toBe("Branch");
    expect(okBody.cases.Some).toBeDefined();
    expect(okBody.cases.None).toBeDefined();
    // Err case: Tag(Err) → Tag(Some)
    const errBody = branch.cases.Err.rest;
    expect(errBody.kind).toBe("Chain");
    expect(errBody.first.handler.builtin.value).toBe("Err");
    expect(errBody.rest.handler.builtin.value).toBe("Some");
  });
});
