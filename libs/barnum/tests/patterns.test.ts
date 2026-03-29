import { describe, it, expect } from "vitest";
import {
  parallel,
  config,
  loop,
  branch,
  pipe,
  forEach,
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
// Parallel
// -----------------------------------------------------------------------

describe("parallel", () => {
  it("accepts actions with the same input type", () => {
    const workflow = parallel(verify, verify);
    expect(workflow.kind).toBe("Parallel");
  });

  it("rejects actions with different input types", () => {
    // setup expects { project: string }, verify expects { artifact: string }
    // @ts-expect-error — input types do not unify
    parallel(setup, verify);
  });

  it("composes with parallel and branch", () => {
    const cfg = config(
      pipe(
        constant({ project: "test" }),
        parallel(
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
// parallel(identity(), handler) → merge()
// Preserves the original input alongside the handler's output.
// -----------------------------------------------------------------------

describe("reader monad pattern", () => {
  it("preserves context via parallel + identity + merge", () => {
    const cfg = config(
      pipe(
        constant({ initialized: true, project: "test" }),
        parallel(identity<{ initialized: boolean; project: string }>(), build),
        merge<[{ initialized: boolean; project: string }, { artifact: string }]>(),
      ),
    );
    expect(cfg.workflow.kind).toBe("Chain");
  });
});
