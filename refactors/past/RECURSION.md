const cfg = configBuilder()
  .registerSteps((others) => ({
    // 'others' contains Step references for both Writer and Reviewer
    Writer: pipe(
      invoke("./writer.ts", "draft"),
      others.Reviewer // Mutual recursion: jump to Reviewer
    ),
    Reviewer: pipe(
      invoke("./reviewer.ts", "critique"),
      branch(isApproved, {
        Approved: invoke("./publisher.ts", "publish"),
        Rejected: others.Writer // Mutual recursion: jump back to Writer
      })
    )
  }));

  Likewise for the workflow parameter. 

  The gist here is that we need to enable recursion, and we should be able to pass steps in as part of the `registerSteps` and `workflow` parameter. In particular, the `workflow` parameter should also accept a self parameter. I'm not sure what best to call it. In any case, the `registerSteps` ones should also, like the others, have the correct type based upon what is passed in as part of `registerSteps`. 