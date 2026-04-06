import Link from '@docusaurus/Link';
import CodeBlock from '@theme/CodeBlock';
import Layout from '@theme/Layout';
import HomepageHeader from '../components/Header';
import styles from './index.module.css';

const handlersExample = `// handlers/steps.ts
import { createHandler } from "@barnum/barnum";
import { z } from "zod";

export const listFiles = createHandler({
  outputValidator: z.array(z.string()),
  handle: async () => {
    return readdirSync("src/").filter(f => f.endsWith(".ts"));
  },
}, "listFiles");

export const refactor = createHandler({
  inputValidator: z.string(),
  handle: async ({ value: file }) => {
    await callAgent({
      prompt: \`Refactor \${file} to replace all class-based React
components with functional components using hooks.\`,
      allowedTools: ["Read", "Edit"],
    });
  },
}, "refactor");

// ... typeCheck, fix, commit, createPR`;

const workflowExample = `// run.ts
import { runPipeline, pipe } from "@barnum/barnum";
import {
  listFiles, refactor, typeCheck, fix, commit, createPR,
} from "./handlers/steps.js";

runPipeline(
  listFiles
    .forEach(pipe(refactor, typeCheck, fix, commit, createPR))
    .drop(),
);`;

const advancedExample = `const refactorWithRetry = pipe(
  refactor,
  evaluate,
  loop((recur) =>
    pipe(typeCheck, classifyErrors).branch({
      HasErrors: pipe(forEach(fix).drop(), recur),
      Clean: drop,
    })
  ),
  commit,
  createPR,
);

runPipeline(
  listFiles.forEach(refactorWithRetry).drop(),
);`;

function Features() {
  return (
    <section className="alt-background">
      <div className="container">
        <h2 className={styles.centeredHeading}>The missing programming language for orchestrating AI agents.</h2>
        <p>
          LLMs are incredibly powerful tools. They are being asked to perform
          increasingly complicated, long-lived tasks. Unfortunately, the naive
          way to work with agents quickly hits limits. When their context
          becomes too full, they become forgetful and make the wrong decisions.
          You can't rely on them to faithfully execute a complicated,
          multi-step plan.
        </p>
        <p>
          Barnum is an attempt to enable LLMs to perform dramatically more
          complicated, ambitious tasks. With Barnum, you define an asynchronous
          workflow, which is effectively a state machine. This makes it easy to
          reason about the possible states and actions that your agents will be
          asked to perform, and the steps can be independent and small.
        </p>
        <div className="row" style={{ paddingTop: '1.5rem' }}>
          <div className="col col--4">
            <h3>A choreographed show</h3>
            <p>
              Workflows are composed from type-safe primitives:{' '}
              <code>pipe</code>, <code>loop</code>, <code>branch</code>,{' '}
              <code>forEach</code>, <code>tryCatch</code>. First-class
              constructs for orchestration, not prose or ad-hoc scripts.
            </p>
          </div>
          <div className="col col--4">
            <h3>The right performer for each act</h3>
            <p>
              Handlers are either built-in primitives or TypeScript async
              functions. Agents handle the parts that require judgment.
              Deterministic code handles the rest. No LLM needed to list
              files or run a type-checker.
            </p>
          </div>
          <div className="col col--4">
            <h3>No one goes off script</h3>
            <p>
              Each handler runs in its own isolated Node.js subprocess.
              The agent performing a refactor never sees the full workflow —
              just its input and a prompt. Focused context means better
              decisions.
            </p>
          </div>
        </div>
      </div>
    </section>
  );
}

function DemoVideo() {
  return (
    <section>
      <div className="container">
        <h2 className={styles.centeredHeading}>See it in action.</h2>
        <div className={styles.demoVideoWrapper}>
          <iframe
            src="https://www.youtube.com/embed/sNRp7hQub8Y"
            title="Barnum demo"
            allow="encrypted-media; picture-in-picture; web-share"
            allowFullScreen
          />
        </div>
      </div>
    </section>
  );
}

function ExampleSection() {
  return (
    <section className="alt-background">
      <div className="container">
        <h2 className={styles.centeredHeading}>A simple example.</h2>
        <p>
          Handlers are the building blocks of a Barnum workflow. Today,
          handlers are either built-in primitives or exported TypeScript async
          functions. (Support for other languages is planned.) You compose
          them into workflows using combinators like <code>pipe</code>{' '}
          (sequential) and <code>forEach</code> (fan-out).
        </p>
        <div className={styles.codeBlockWrap}>
          <CodeBlock language="ts" title="handlers/steps.ts">
            {handlersExample}
          </CodeBlock>
        </div>
        <div className={styles.codeBlockWrap}>
          <CodeBlock language="ts" title="run.ts">
            {workflowExample}
          </CodeBlock>
        </div>
        <p>
          <code>listFiles</code> runs once and returns an array of filenames.{' '}
          <code>forEach</code> fans out — each filename flows through{' '}
          <code>refactor → typeCheck → fix → commit → createPR</code>,
          with each file processed in parallel.
          Each handler executes in its own isolated subprocess. The Rust
          runtime manages the state machine: dispatching handlers, collecting
          results, and advancing the workflow. No handler sees another
          handler's context.
        </p>
      </div>
    </section>
  );
}

function AdvancedSection() {
  return (
    <section>
      <div className="container">
        <h2 className={styles.centeredHeading}>Why not just write this in JavaScript?</h2>
        <p>
          The simple example above is simple. You could probably ask your
          favorite LLM to one-shot the orchestration script, and it would do
          a decent job. When the workflow grows in complexity, you might reach
          for plan mode or write a markdown file describing the steps. That
          works for a while. But what happens when the plan has 40 steps
          across 15 files with conditional branches, retries on failure,
          parallel fan-out, and a review loop? Good luck getting an agent to
          faithfully and reliably execute that plan.
        </p>
        <p>
          And in practice, you <em>do</em> want the complicated version.
          You want the agent to refactor, then evaluate the result,
          then type-check, then fix errors in a loop until it's clean:
        </p>
        <div className={styles.codeBlockWrap}>
          <CodeBlock language="ts">
            {advancedExample}
          </CodeBlock>
        </div>
        <p>
          The problem isn't that any individual piece is hard. The problem is
          that expressing a precise, complicated asynchronous workflow in prose
          or ad-hoc scripts is fragile. A programming language geared towards
          orchestration is what you actually want — one where{' '}
          <code>loop</code>, <code>branch</code>, <code>tryCatch</code>,{' '}
          <code>forEach</code>, and <code>pipe</code> are first-class
          constructs with type-safe composition.
        </p>
      </div>
    </section>
  );
}

function AgentAuthoring() {
  return (
    <section className="alt-background">
      <div className="container">
        <h2 className={styles.centeredHeading}>Looks complicated? Agents are good at writing this.</h2>
        <p>
          Barnum workflows are TypeScript with strong types and Zod
          validators. Every combinator is fully typed — your agent gets
          autocomplete, type errors, and compiler feedback as it writes.
          Show it one of the{' '}
          <a href="https://github.com/barnum-circus/barnum/tree/master/demos">
            working demos
          </a>{' '}
          as a reference and tell it what you want. It'll write a working
          pipeline.
        </p>
      </div>
    </section>
  );
}

function WhyBarnum() {
  return (
    <section>
      <div className="container">
        <h2 className={styles.centeredHeading}>What Barnum gives you</h2>
        <div className={`row ${styles.patternList}`}>
          <div className="col col--6">
            <ul>
              <li>
                <strong><code>pipe</code></strong>: sequential chains.
                Process steps one after another.
              </li>
              <li>
                <strong><code>forEach</code></strong>: fan-out to parallel.
                List 50 files, refactor them all concurrently.
              </li>
              <li>
                <strong><code>loop</code></strong>: retry and iterate.
                Fix type errors in a loop until the code is clean.
              </li>
              <li>
                <strong><code>branch</code></strong>: conditional routing.
                An analyzer classifies; specialists execute.
              </li>
            </ul>
          </div>
          <div className="col col--6">
            <ul>
              <li>
                <strong><code>tryCatch</code></strong>: error recovery.
                Catch failures and route to fallback handlers.
              </li>
              <li>
                <strong><code>withTimeout</code></strong>: deadline enforcement.
                Time out a handler and fall back to an alternative.
              </li>
              <li>
                <strong>Schema validation</strong>: handlers declare input
                and output schemas via Zod. Validated at every boundary.
              </li>
              <li>
                <strong>Isolated execution</strong>: each handler runs in
                its own subprocess. No shared context, no drift.
              </li>
            </ul>
          </div>
        </div>
      </div>
    </section>
  );
}

function HowItWorks() {
  return (
    <section className="alt-background">
      <div className="container">
        <h2 className={styles.centeredHeading}>Ladies and gentlemen, the show is about to begin!</h2>
        <div className={styles.ctaContainer}>
          <Link
            className="button button--primary button--lg"
            to="/docs/quickstart"
          >
            I'm ready, start the show
          </Link>
        </div>
      </div>
    </section>
  );
}

export default function Home(): JSX.Element {
  return (
    <Layout
      title="Barnum - The programming language for orchestrating agents"
      description="Barnum is a programming language for asynchronous programming that is geared towards making it easy to precisely orchestrate agents."
    >
      <HomepageHeader />
      <main>
        <Features />
        <DemoVideo />
        <ExampleSection />
        <AdvancedSection />
        <AgentAuthoring />
        <WhyBarnum />
        <HowItWorks />
      </main>
    </Layout>
  );
}
