import CodeBlock from '@theme/CodeBlock';
import Layout from '@theme/Layout';
import HomepageHeader from '../components/Header';
import styles from './index.module.css';

const exampleConfig = `{
  "entrypoint": "ListFiles",
  "steps": [
    {
      "name": "ListFiles",
      // One ConvertToTS task per .js file
      "action": {
        "kind": "Command",
        "script": "find src -name '*.js' | jq -R '{kind: \\"ConvertToTS\\", value: {file: .}}' | jq -s '.'"
      },
      "next": ["ConvertToTS"],
      // After all conversions: fix any remaining type errors
      "finally": "echo '[{\\"kind\\": \\"FixErrors\\", \\"value\\": {}}]'"
    },
    {
      "name": "ConvertToTS",
      "value_schema": {
        "type": "object",
        "required": ["file"],
        "properties": { "file": { "type": "string" } }
      },
      "action": {
        "kind": "Pool",
        "instructions": {
          "inline": "Convert this JS file to TypeScript. Add types, rename to .ts. Return []."
        }
      },
      "next": []
    },
    {
      "name": "FixErrors",
      "action": {
        "kind": "Pool",
        "instructions": {
          "inline": "Run npx tsc --noEmit and fix all TypeScript errors. Return []."
        }
      },
      "next": []
    }
  ]
}`;

function Features() {
  return (
    <section className="alt-background">
      <div className="container padding-vert--lg">
        <div className="row">
          <div className="col col--4">
            <h3>Rigorous workflows</h3>
            <p>
              Express workflows as statically analyzable state machines.
              Valid transitions are declared upfront. Invalid ones are
              rejected and retried. No hoping the agent stays on track.
            </p>
          </div>
          <div className="col col--4">
            <h3>Mix agents and commands</h3>
            <p>
              Intersperse LLM steps with local shell commands for
              deterministic operations. Fan-out with jq, commit with git,
              validate with your compiler — no agent needed.
            </p>
          </div>
          <div className="col col--4">
            <h3>Context protection</h3>
            <p>
              Each step gets only the instructions and data it needs.
              Agents never see the full workflow — just their current task.
              Focused context means better decisions.
            </p>
          </div>
        </div>
      </div>
    </section>
  );
}

function ExampleSection() {
  return (
    <section>
      <div className="container padding-vert--lg">
        <h2>One config. Complex workflows.</h2>
        <p>
          A command lists every <code>.js</code> file. GSD dispatches one
          agent per file to convert it to TypeScript — in parallel.
          When all conversions finish, a <code>finally</code> hook
          triggers an agent that runs <code>tsc</code> and fixes any
          remaining type errors. One JSON file, no glue code.
        </p>
        <div className="row">
          <div className="col col--6">
            <div className={styles.codeBlockWrap}>
              <CodeBlock language="json" title="config.jsonc">
                {exampleConfig}
              </CodeBlock>
            </div>
          </div>
          <div className={`col col--6 ${styles.demoPlaceholder}`}>
            <div className={styles.demoPlaceholderInner}>
              <p className={styles.demoPlaceholderTitle}>
                asciinema demo coming soon
              </p>
              <p className={styles.demoPlaceholderSubtitle}>
                Watch GSD orchestrate a multi-file refactor in real time
              </p>
            </div>
          </div>
        </div>
        <p className={styles.agentNote}>
          Looks complicated? Agents can help you write these configs too.
        </p>
      </div>
    </section>
  );
}

function WhyGSD() {
  return (
    <section className="alt-background">
      <div className="container padding-vert--lg">
        <h2>
          Why GSD? <span className={styles.gsdSubtitle}>(Get Sh*** Done)</span>
        </h2>
        <p>
          A single agent with a markdown plan can handle simple tasks. But
          real work — migrating 50 files, refactoring across a codebase,
          running multi-step pipelines — breaks that model fast. Context
          fills up, the agent loses track, and you can't predict what it
          will do before you run it.
        </p>
        <p>
          GSD is like a build system for agents. You declare the full graph of
          steps and valid transitions upfront — it's statically analyzable
          before anything runs. At runtime, agents choose which path through the
          graph to take, but they can never leave the rails.
        </p>
        <h3>What GSD gives you</h3>
        <div className={`row ${styles.patternList}`}>
          <div className="col col--6">
            <ul>
              <li>
                <strong>Fan-out</strong> — split work into parallel tasks.
                List 50 files, refactor them all concurrently, commit when done.
              </li>
              <li>
                <strong>Branching</strong> — route to different agents based
                on what the code needs. An analyzer decides; a specialist executes.
              </li>
              <li>
                <strong>Sequential chains</strong> — process items one at a time
                when order matters, like applying multiple changes to the same file.
              </li>
              <li>
                <strong>Adversarial review</strong> — implement, then judge, then
                revise. Loop until a critic agent approves the work.
              </li>
            </ul>
          </div>
          <div className="col col--6">
            <ul>
              <li>
                <strong>Error recovery</strong> — post hooks catch failures and
                route them to fix-up agents instead of just retrying blindly.
              </li>
              <li>
                <strong>Hooks</strong> — enrich context before an agent sees it,
                validate results after, clean up resources when a subtree completes.
              </li>
              <li>
                <strong>Schema validation</strong> — every step declares what data
                it accepts. Malformed responses are rejected before they propagate.
              </li>
              <li>
                <strong>Commands</strong> — deterministic shell scripts for the
                mechanical parts: listing files, calling APIs, running builds.
                Save the LLM for the thinking.
              </li>
            </ul>
          </div>
        </div>
        <p className={styles.closingNote}>
          Each pattern is a JSON config — no framework, no SDK, no
          custom language. Define the state machine, point it at an agent pool,
          and let GSD handle the orchestration.
        </p>
      </div>
    </section>
  );
}

function HowItWorks() {
  return (
    <section>
      <div className="container padding-vert--lg">
        <h2>How it works</h2>
        <div className="row">
          <div className="col col--4">
            <h3>1. Define</h3>
            <p>
              Write a JSON config with steps, transitions, and schemas.
              Each step is either an agent task or a shell command.
              GSD validates the config before anything runs.
            </p>
          </div>
          <div className="col col--4">
            <h3>2. Run</h3>
            <p>
              Start an agent pool, then run your workflow. GSD dispatches
              tasks to agents, enforces valid transitions, retries failures,
              and respects concurrency limits.
            </p>
          </div>
          <div className="col col--4">
            <h3>3. Scale</h3>
            <p>
              Add more agents to the pool for parallel throughput.
              The same config works whether you have 1 agent or 20.
              Each agent only sees its current task — context stays clean.
            </p>
          </div>
        </div>
      </div>
    </section>
  );
}

export default function Home(): JSX.Element {
  return (
    <Layout
      title="GSD - The missing workflow engine for agents"
      description="Don't just /loop it. GSD is the missing workflow engine for agents — define complex trees of work as statically analyzable state machines."
    >
      <HomepageHeader />
      <main>
        <Features />
        <ExampleSection />
        <WhyGSD />
        <HowItWorks />
      </main>
    </Layout>
  );
}
