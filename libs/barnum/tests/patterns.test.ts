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
  constant,
  drop,
  identity,
  merge,
  tag,
  Option as O,
  Result as R,
} from "../src/index.js";
import type { OptionDef, ResultDef } from "../src/ast.js";

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

/**
 * Build the expected AST for `tag(kind)`.
 *
 * tag(kind) composes to:
 * Chain(All(Chain(Constant(kind), WrapInField("kind")), WrapInField("value")), Merge())
 */
function expectedTagAst(kind: string) {
  return {
    kind: "Chain",
    first: {
      kind: "All",
      actions: [
        {
          kind: "Chain",
          first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Constant", value: kind } } },
          rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "WrapInField", field: "kind" } } },
        },
        { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "WrapInField", field: "value" } } },
      ],
    },
    rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Merge" } } },
  };
}

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
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("produces Chain(tag(Continue), RestartHandle(...)) AST", () => {
    const workflow = loop<{ stable: true }, { deployed: boolean }>((recur, done) =>
      healthCheck.branch({ Continue: recur, Break: done }),
    );
    expect(workflow.kind).toBe("Chain");
    const chain = workflow as any;
    // First: tag("Continue") — now a composed Chain(All(...), Merge)
    expect(chain.first).toEqual(expectedTagAst("LoopResult.Continue"));
    // Rest: RestartHandle
    expect(chain.rest.kind).toBe("RestartHandle");
    expect(typeof chain.rest.restart_handler_id).toBe("number");
    // RestartHandle body: Branch with Continue/Break cases
    expect(chain.rest.body.kind).toBe("Branch");
    expect(Object.keys(chain.rest.body.cases).toSorted()).toEqual(["Break", "Continue"]);
    // RestartHandle handler: Chain(Invoke(GetIndex(0)), Branch(unwrap))
    expect(chain.rest.handler.kind).toBe("Chain");
    expect(chain.rest.handler.first.kind).toBe("Invoke");
    expect(chain.rest.handler.first.handler.builtin.kind).toBe("GetIndex");
    expect(chain.rest.handler.first.handler.builtin.index).toBe(0);
  });

  it("composes type-check loop with branch", () => {
    const cfg = config(
      pipe(
        constant({ project: "test" }),
        setup,
        listFiles,
        forEach(migrate),
      ).then(loop((recur, done) =>
        pipe(
          typeCheck,
          classifyErrors,
        ).branch({
          HasErrors: pipe(forEach(fix).drop(), recur),
          Clean: done,
        }),
      )),
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
      HasErrors: drop,
      Clean: drop,
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

  it(".tag() produces Chain → tag composition AST", () => {
    const action = verify.tag<"VerifyResult", { Ok: { verified: boolean } }, "Ok">("Ok", "VerifyResult");
    expect(action.kind).toBe("Chain");
    const chain = action as { kind: "Chain"; first: any; rest: any };
    expect(chain.first.kind).toBe("Invoke");
    // rest is the tag("Ok") composition
    expect(chain.rest).toEqual(expectedTagAst("VerifyResult.Ok"));
  });

  it(".getField() produces Chain → GetField AST", () => {
    const action = setup.getField("project");
    expect(action.kind).toBe("Chain");
    const chain = action as { kind: "Chain"; first: any; rest: any };
    expect(chain.first.kind).toBe("Invoke");
    expect(chain.rest.kind).toBe("Invoke");
    expect(chain.rest.handler.builtin.kind).toBe("GetField");
    expect(chain.rest.handler.builtin.field).toBe("project");
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
      Clean: drop,
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
  it("Option.map() produces Branch with Some and None cases", () => {
    const action = O.map(verify);
    expect(action.kind).toBe("Branch");
    const branchNode =action as { kind: "Branch"; cases: any };
    expect(Object.keys(branchNode.cases).toSorted()).toEqual(["None", "Some"]);
    // Some case: GetField("value") → Chain(verify, tag("Some"))
    const someCase = branchNode.cases["Some"];
    expect(someCase.kind).toBe("Chain");
    expect(someCase.first.handler.builtin.kind).toBe("GetField");
    expect(someCase.rest.kind).toBe("Chain");
    // None case: GetField("value") → tag("None") composition
    const noneCase = branchNode.cases["None"];
    expect(noneCase.kind).toBe("Chain");
    expect(noneCase.rest).toEqual(expectedTagAst("Option.None"));
  });

  it("Option.andThen() produces Branch with action Some and tag None", () => {
    const action = O.andThen(pipe(verify, tag<"Option", OptionDef<{ verified: boolean }>, "Some">("Some", "Option")));
    expect(action.kind).toBe("Branch");
    const branchNode =action as { kind: "Branch"; cases: any };
    expect(Object.keys(branchNode.cases).toSorted()).toEqual(["None", "Some"]);
    // Some case: GetField("value") → Chain(verify, tag("Some"))
    const someCase = branchNode.cases["Some"];
    expect(someCase.kind).toBe("Chain");
    expect(someCase.first.handler.builtin.kind).toBe("GetField");
    // None case: GetField("value") → tag("None") composition
    const noneCase = branchNode.cases["None"];
    expect(noneCase.rest).toEqual(expectedTagAst("Option.None"));
  });

  it("Option.unwrapOr() produces Branch with identity Some and default None", () => {
    const action = O.unwrapOr(constant("fallback"));
    expect(action.kind).toBe("Branch");
    const branchNode =action as { kind: "Branch"; cases: any };
    // Some case: GetField("value") → Identity
    const someCase = branchNode.cases["Some"];
    expect(someCase.rest.handler.builtin.kind).toBe("Identity");
    // None case: GetField("value") → Constant("fallback")
    const noneCase = branchNode.cases["None"];
    expect(noneCase.rest.handler.builtin.kind).toBe("Constant");
    expect(noneCase.rest.handler.builtin.value).toBe("fallback");
  });

  it("Option.flatten() produces Branch with identity Some and tag None", () => {
    const action = O.flatten<string>();
    expect(action.kind).toBe("Branch");
    const branchNode =action as { kind: "Branch"; cases: any };
    expect(branchNode.cases["Some"].rest.handler.builtin.kind).toBe("Identity");
    expect(branchNode.cases["None"].rest).toEqual(expectedTagAst("Option.None"));
  });

  it("Option.filter() produces Branch with predicate Some and tag None", () => {
    const predicate = tag<"Option", OptionDef<string>, "Some">("Some", "Option");
    const action = O.filter(predicate);
    expect(action.kind).toBe("Branch");
    const branchNode =action as { kind: "Branch"; cases: any };
    // Some case body is the predicate (tag "Some" composition)
    expect(branchNode.cases["Some"].rest).toEqual(expectedTagAst("Option.Some"));
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
    const branchNode =action as { kind: "Branch"; cases: any };
    // Some → Constant(true)
    const someCase = branchNode.cases["Some"].rest;
    expect(someCase.handler.builtin.kind).toBe("Constant");
    expect(someCase.handler.builtin.value).toBe(true);
    // None → Constant(false)
    const noneCase = branchNode.cases["None"].rest;
    expect(noneCase.handler.builtin.kind).toBe("Constant");
    expect(noneCase.handler.builtin.value).toBe(false);
  });

  it("Option.isNone() is the inverse of isSome", () => {
    const action = O.isNone<string>();
    expect(action.kind).toBe("Branch");
    const branchNode =action as { kind: "Branch"; cases: any };
    // Some → Constant(false)
    expect(branchNode.cases["Some"].rest.handler.builtin.value).toBe(false);
    // None → Constant(true)
    expect(branchNode.cases["None"].rest.handler.builtin.value).toBe(true);
  });

});

// -----------------------------------------------------------------------
// Reader monad pattern
//
// all(identity, handler) → merge()
// Preserves the original input alongside the handler's output.
// -----------------------------------------------------------------------

describe("reader monad pattern", () => {
  it("preserves context via all + identity + merge", () => {
    const cfg = config(
      pipe(
        constant({ initialized: true, project: "test" }),
        all(identity(), build),
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

  it("single binding produces Chain(All(..., Identity), ResumeHandle(readVar, Chain(GetIndex, body)))", () => {
    const exprA = constant(42);
    const bodyAction = identity();
    const result = bind([exprA], ([_a]) => bodyAction);

    // Outer: Chain
    expect(result.kind).toBe("Chain");
    const outer = result as { kind: "Chain"; first: any; rest: any };

    // First: All with 2 actions (binding + Identity)
    expect(outer.first.kind).toBe("All");
    expect(outer.first.actions).toHaveLength(2);
    expect(outer.first.actions[0]).toEqual(exprA);
    expect(outer.first.actions[1].handler.builtin.kind).toBe("Identity");

    // Rest: ResumeHandle
    expect(outer.rest.kind).toBe("ResumeHandle");
    const handle = outer.rest as { kind: "ResumeHandle"; resume_handler_id: number; handler: any; body: any };
    expect(typeof handle.resume_handler_id).toBe("number");

    // ResumeHandle handler: readVar(0) = All(Chain(GetIndex(1).unwrap(), GetIndex(0).unwrap()), GetIndex(1).unwrap())
    // Each getIndex(n).unwrap() is Chain(Invoke(GetIndex(n)), Branch(unwrap))
    expect(handle.handler.kind).toBe("All");
    expect(handle.handler.actions).toHaveLength(2);
    // First action: Chain(Chain(GetIndex(1), unwrap), Chain(GetIndex(0), unwrap))
    expect(handle.handler.actions[0].kind).toBe("Chain");
    expect(handle.handler.actions[0].first.kind).toBe("Chain");
    expect(handle.handler.actions[0].first.first.handler.builtin.kind).toBe("GetIndex");
    expect(handle.handler.actions[0].first.first.handler.builtin.index).toBe(1);
    expect(handle.handler.actions[0].rest.kind).toBe("Chain");
    expect(handle.handler.actions[0].rest.first.handler.builtin.kind).toBe("GetIndex");
    expect(handle.handler.actions[0].rest.first.handler.builtin.index).toBe(0);
    // Second action: Chain(GetIndex(1), unwrap)
    expect(handle.handler.actions[1].kind).toBe("Chain");
    expect(handle.handler.actions[1].first.handler.builtin.kind).toBe("GetIndex");
    expect(handle.handler.actions[1].first.handler.builtin.index).toBe(1);

    // ResumeHandle body: Chain(Chain(GetIndex(1), unwrap), bodyAction)
    expect(handle.body.kind).toBe("Chain");
    expect(handle.body.first.kind).toBe("Chain");
    expect(handle.body.first.first.handler.builtin.kind).toBe("GetIndex");
    expect(handle.body.first.first.handler.builtin.index).toBe(1);
  });

  it("two bindings produce two nested Handles with distinct effectIds", () => {
    const exprA = constant("alice");
    const exprB = constant(99);
    const bodyAction = identity();
    const result = bind([exprA, exprB], ([_a, _b]) => bodyAction);

    const outer = result as { kind: "Chain"; first: any; rest: any };

    // All with 3 actions (2 bindings + Identity)
    expect(outer.first.kind).toBe("All");
    expect(outer.first.actions).toHaveLength(3);

    // Outer ResumeHandle
    const handle0 = outer.rest;
    expect(handle0.kind).toBe("ResumeHandle");

    // Inner ResumeHandle
    const handle1 = handle0.body;
    expect(handle1.kind).toBe("ResumeHandle");

    // Distinct resume_handler_ids
    expect(handle0.resume_handler_id).not.toBe(handle1.resume_handler_id);

    // readVar indices: outer=0, inner=1
    // Each getIndex(n).unwrap() is Chain(Invoke(GetIndex(n)), Branch(unwrap))
    // readVar's first action is Chain(getIndex(1).unwrap(), getIndex(n).unwrap())
    // So .rest is getIndex(n).unwrap() = Chain(Invoke(GetIndex(n)), Branch(unwrap))
    expect(handle0.handler.actions[0].rest.first.handler.builtin.index).toBe(0);
    expect(handle1.handler.actions[0].rest.first.handler.builtin.index).toBe(1);

    // Innermost body: Chain(getIndex(2).unwrap(), bodyAction) — pipeline_input at index 2
    expect(handle1.body.kind).toBe("Chain");
    expect(handle1.body.first.kind).toBe("Chain");
    expect(handle1.body.first.first.handler.builtin.kind).toBe("GetIndex");
    expect(handle1.body.first.first.handler.builtin.index).toBe(2);
  });

  it("VarRef is a ResumePerform node with unique resume_handler_id", () => {
    const exprA = constant("x");
    let capturedVarRef: any;
    bind([exprA], ([a]) => {
      capturedVarRef = a;
      return identity();
    });

    expect(capturedVarRef.kind).toBe("ResumePerform");
    expect(typeof capturedVarRef.resume_handler_id).toBe("number");
  });

  it("resume_handler_ids are unique across separate bind calls", () => {
    const resumeHandlerIds: number[] = [];
    bind([constant(1), constant(2)], ([_a, _b]) => identity());
    // First bind uses resume_handler_ids 0, 1

    let ref1: any, ref2: any;
    bind([constant(3), constant(4)], ([a, b]) => {
      ref1 = a;
      ref2 = b;
      return identity();
    });
    // Second bind uses resume_handler_ids 2, 3
    resumeHandlerIds.push(ref1.resume_handler_id, ref2.resume_handler_id);

    // All four resume_handler_ids (0, 1, 2, 3) are distinct
    expect(ref1.resume_handler_id).not.toBe(0);
    expect(ref1.resume_handler_id).not.toBe(1);
    expect(ref2.resume_handler_id).not.toBe(0);
    expect(ref2.resume_handler_id).not.toBe(1);
    expect(ref1.resume_handler_id).not.toBe(ref2.resume_handler_id);
  });

  it("readVar(n) structure is All(Chain(GetIndex(1).unwrap(), GetIndex(n).unwrap()), GetIndex(1).unwrap())", () => {
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
      expect(handler.kind).toBe("All");
      expect(handler.actions).toHaveLength(2);
      // First action: Chain(Chain(GetIndex(1), unwrap), Chain(GetIndex(n), unwrap))
      // Each getIndex(x).unwrap() is Chain(Invoke(GetIndex(x)), Branch(unwrap))
      const chainAction = handler.actions[0];
      expect(chainAction.kind).toBe("Chain");
      expect(chainAction.first.kind).toBe("Chain");
      expect(chainAction.first.first.handler.builtin.kind).toBe("GetIndex");
      expect(chainAction.first.first.handler.builtin.index).toBe(1);
      expect(chainAction.rest.kind).toBe("Chain");
      expect(chainAction.rest.first.handler.builtin.kind).toBe("GetIndex");
      expect(chainAction.rest.first.handler.builtin.index).toBe(expectedIndex);
      // Second action: Chain(GetIndex(1), unwrap)
      const extractAction = handler.actions[1];
      expect(extractAction.kind).toBe("Chain");
      expect(extractAction.first.handler.builtin.kind).toBe("GetIndex");
      expect(extractAction.first.handler.builtin.index).toBe(1);
    }
  });
});

describe("bindInput", () => {
  beforeEach(() => {
    resetEffectIdCounter();
  });

  it("compiles to bind([identity], ([input]) => pipe(drop, body(input)))", () => {
    const bodyAction = constant("result");
    const result = bindInput<string, string>((_input) => bodyAction);

    // Outer: Chain(All(Identity, Identity), Handle(...))
    const outer = result as { kind: "Chain"; first: any; rest: any };
    expect(outer.first.kind).toBe("All");
    expect(outer.first.actions).toHaveLength(2);
    // First action is identity (from bind([identity], ...))
    expect(outer.first.actions[0].handler.builtin.kind).toBe("Identity");
    // Second action is identity (pipeline input preservation)
    expect(outer.first.actions[1].handler.builtin.kind).toBe("Identity");

    // ResumeHandle wraps the body
    expect(outer.rest.kind).toBe("ResumeHandle");
    const handle = outer.rest;

    // Handle body: Chain(Chain(GetIndex(1), unwrap), Chain(Drop, bodyAction))
    // getIndex(1).unwrap() is Chain(Invoke(GetIndex(1)), Branch(unwrap))
    expect(handle.body.kind).toBe("Chain");
    expect(handle.body.first.kind).toBe("Chain");
    expect(handle.body.first.first.handler.builtin.kind).toBe("GetIndex");
    expect(handle.body.first.first.handler.builtin.index).toBe(1);

    // The rest of the body is Chain(Drop, bodyAction)
    const bodyChain = handle.body.rest;
    expect(bodyChain.kind).toBe("Chain");
    expect(bodyChain.first.handler.builtin.kind).toBe("Drop");
  });

  it("VarRef from bindInput is a ResumePerform node", () => {
    let capturedRef: any;
    bindInput<string, string>((input) => {
      capturedRef = input;
      return constant("result");
    });

    expect(capturedRef.kind).toBe("ResumePerform");
    expect(typeof capturedRef.resume_handler_id).toBe("number");
  });
});

// ---------------------------------------------------------------------------
// Result namespace — AST structure tests
// ---------------------------------------------------------------------------

describe("Result combinators", () => {
  it("Result.map(action) desugars to Branch(Ok: Chain(action, tag(Ok)), Err: tag(Err))", () => {
    const action = R.map(setup);
    expect(action).toEqual({
      kind: "Branch",
      cases: {
        Ok: {
          kind: "Chain",
          first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "GetField", field: "value" } } },
          rest: { kind: "Chain", first: setup, rest: expectedTagAst("Result.Ok") },
        },
        Err: {
          kind: "Chain",
          first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "GetField", field: "value" } } },
          rest: expectedTagAst("Result.Err"),
        },
      },
    });
  });

  it("Result.mapErr(action) desugars to Branch(Ok: tag(Ok), Err: Chain(action, tag(Err)))", () => {
    const action = R.mapErr(setup);
    expect(action).toEqual({
      kind: "Branch",
      cases: {
        Ok: {
          kind: "Chain",
          first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "GetField", field: "value" } } },
          rest: expectedTagAst("Result.Ok"),
        },
        Err: {
          kind: "Chain",
          first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "GetField", field: "value" } } },
          rest: { kind: "Chain", first: setup, rest: expectedTagAst("Result.Err") },
        },
      },
    });
  });

  it("Result.andThen(action) desugars to Branch(Ok: action, Err: tag(Err))", () => {
    const inner = tag<"Result", ResultDef<string, string>, "Ok">("Ok", "Result");
    const result = R.andThen(inner);
    const branchNode =result as any;
    expect(branchNode.kind).toBe("Branch");
    // Ok case: ExtractValue → action
    expect(branchNode.cases.Ok.rest).toEqual(inner);
    // Err case: ExtractValue → tag(Err) composition
    expect(branchNode.cases.Err.rest).toEqual(expectedTagAst("Result.Err"));
  });

  it("Result.or(fallback) desugars to Branch(Ok: tag(Ok), Err: fallback)", () => {
    const fallback = tag<"Result", ResultDef<string, string>, "Ok">("Ok", "Result");
    const result = R.or(fallback);
    const branchNode =result as any;
    expect(branchNode.kind).toBe("Branch");
    expect(branchNode.cases.Ok.rest).toEqual(expectedTagAst("Result.Ok"));
    expect(branchNode.cases.Err.rest).toBe(fallback);
  });

  it("Result.and(other) desugars to Branch(Ok: Chain(Drop, other), Err: Tag(Err))", () => {
    const other = pipe(constant("replacement"), tag<"Result", ResultDef<string, string>, "Ok">("Ok", "Result"));
    const result = R.and(other);
    const branchNode =result as any;
    expect(branchNode.kind).toBe("Branch");
    // Ok case: ExtractValue → Chain(Drop, other)
    const okBody = branchNode.cases.Ok.rest;
    expect(okBody.kind).toBe("Chain");
    expect(okBody.first.handler.builtin.kind).toBe("Drop");
    expect(okBody.rest).toBe(other);
  });

  it("Result.unwrapOr(default) desugars to Branch(Ok: Identity, Err: default)", () => {
    const fallback = constant("default");
    const action = R.unwrapOr(fallback);
    const branchNode =action as any;
    expect(branchNode.kind).toBe("Branch");
    expect(branchNode.cases.Ok.rest.handler.builtin.kind).toBe("Identity");
    expect(branchNode.cases.Err.rest).toBe(fallback);
  });

  it("Result.flatten() desugars to Branch(Ok: Identity, Err: tag(Err))", () => {
    const action = R.flatten();
    const branchNode =action as any;
    expect(branchNode.kind).toBe("Branch");
    expect(branchNode.cases.Ok.rest.handler.builtin.kind).toBe("Identity");
    expect(branchNode.cases.Err.rest).toEqual(expectedTagAst("Result.Err"));
  });

  it("Result.toOption() desugars to Branch(Ok: tag(Some), Err: Chain(Drop, tag(None)))", () => {
    const action = R.toOption();
    const branchNode =action as any;
    expect(branchNode.kind).toBe("Branch");
    expect(branchNode.cases.Ok.rest).toEqual(expectedTagAst("Option.Some"));
    expect(branchNode.cases.Err.rest.first.handler.builtin.kind).toBe("Drop");
    expect(branchNode.cases.Err.rest.rest).toEqual(expectedTagAst("Option.None"));
  });

  it("Result.toOptionErr() desugars to Branch(Ok: Chain(Drop, tag(None)), Err: tag(Some))", () => {
    const action = R.toOptionErr();
    const branchNode =action as any;
    expect(branchNode.kind).toBe("Branch");
    expect(branchNode.cases.Ok.rest.first.handler.builtin.kind).toBe("Drop");
    expect(branchNode.cases.Ok.rest.rest).toEqual(expectedTagAst("Option.None"));
    expect(branchNode.cases.Err.rest).toEqual(expectedTagAst("Option.Some"));
  });

  it("Result.isOk() desugars to Branch(Ok: Constant(true), Err: Constant(false))", () => {
    const action = R.isOk();
    const branchNode =action as any;
    expect(branchNode.kind).toBe("Branch");
    expect(branchNode.cases.Ok.rest.handler.builtin.value).toBe(true);
    expect(branchNode.cases.Err.rest.handler.builtin.value).toBe(false);
  });

  it("Result.isErr() desugars to Branch(Ok: Constant(false), Err: Constant(true))", () => {
    const action = R.isErr();
    const branchNode =action as any;
    expect(branchNode.kind).toBe("Branch");
    expect(branchNode.cases.Ok.rest.handler.builtin.value).toBe(false);
    expect(branchNode.cases.Err.rest.handler.builtin.value).toBe(true);
  });

  it("Result.transpose() desugars to nested branches", () => {
    const action = R.transpose();
    const branchNode =action as any;
    expect(branchNode.kind).toBe("Branch");
    // Ok case: receives Option, branches on Some/None
    const okBody = branchNode.cases.Ok.rest;
    expect(okBody.kind).toBe("Branch");
    expect(okBody.cases.Some).toBeDefined();
    expect(okBody.cases.None).toBeDefined();
    // Err case: tag(Err) → tag(Some) — now both are compositions
    const errBody = branchNode.cases.Err.rest;
    expect(errBody.kind).toBe("Chain");
    expect(errBody.first).toEqual(expectedTagAst("Result.Err"));
    expect(errBody.rest).toEqual(expectedTagAst("Option.Some"));
  });
});
