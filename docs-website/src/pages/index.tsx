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
        <h2 className={styles.centeredHeading}>The missing workflow engine for AI agents.</h2>
        <div className="row">
          <div className="col col--4">
            <h3>🦁 No one leaves the ring</h3>
            <p>
              Rigorous workflows expressed as statically analyzable state machines.
              Transitions are declared upfront and validated when the
              workflow is run. Invalid ones are rejected and retried.
              No hoping the agent stays on track.
            </p>
          </div>
          <div className="col col--4">
            <h3>🐘 The right performer for each act</h3>
            <p>
              Some acts are agents, some acts are shell commands, and each
              does what it's best at. Fan-out with <code>jq</code>,
              commit with <code>git</code>, validate with your
              compiler, no agent needed.
            </p>
          </div>
          <div className="col col--4">
            <h3>🐯 One act at a time</h3>
            <p>
              Each step gets only the instructions and data it needs.
              Agents never see the full workflow, just their current task.
              Focused context means agents can make better decisions.
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
        <h2 className={styles.centeredHeading}>One programme. Greatest show on earth.</h2>
        <p>
          With Barnum, you specify your workflow upfront in a configuration
          file. You can express ordering constraints (A before B), fan-out
          (one task per file), and aggregation (do X after everything
          finishes) in plain, readable JSON that can be validated before
          anything runs. Agents only handle the parts that require judgment.
          They never see the full workflow, so their context stays small and
          they don't drift off course as the work scales up. Each agent
          response is validated against a schema you define, so the workflow
          executes exactly as you specified.
        </p>
        <p>
          In this programme, a command lists each <code>.js</code> file.
          Barnum dispatches one agent per file to convert it to TypeScript,
          in parallel. When all conversions finish, a <code>finally</code>{' '}
          hook triggers an agent that runs <code>tsc</code> and fixes any
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
                Watch Barnum orchestrate a multi-file refactor in real time
              </p>
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}

function AgentAuthoring() {
  return (
    <section className="alt-background">
      <div className="container padding-vert--lg">
        <h2 className={styles.centeredHeading}>Looks complicated? Let the performers write the programme.</h2>
        <p>
          Barnum programmes are just JSON with a{' '}
          <a href="/docs/reference/config-schema">published schema</a>.
          Point your agent at{' '}
          <code>pnpm dlx @barnum/barnum config schema</code> to get
          the full JSON Schema, show it the{' '}
          <a href="/docs/repertoire">repertoire</a> for common
          patterns, and tell it what you want. It'll write a working
          programme.
        </p>
      </div>
    </section>
  );
}

function WhyBarnum() {
  return (
    <section>
      <div className="container padding-vert--lg">
        <h2 className={styles.centeredHeading}>Why Barnum?</h2>
        <p>
          A single agent with a markdown plan can handle simple tasks. But
          real work (migrating 50 files, refactoring across a codebase,
          running multi-step pipelines) breaks that model fast. Context
          fills up, the agent loses track, and you can't predict what it
          will do before you run it.
        </p>
        <p>
          Barnum is the ringmaster for your agents. You declare the full graph of
          steps and valid transitions upfront. It's statically analyzable
          before anything runs. At runtime, agents choose which path through the
          graph to take, but they can never leave the rails.
        </p>
        <h3>What Barnum gives you</h3>
        <div className={`row ${styles.patternList}`}>
          <div className="col col--6">
            <ul>
              <li>
                <strong>Fan-out</strong>: split work into parallel tasks.
                List 50 files, refactor them all concurrently, commit when done.
              </li>
              <li>
                <strong>Branching</strong>: route to different agents based
                on what the code needs. An analyzer decides; a specialist executes.
              </li>
              <li>
                <strong>Sequential chains</strong>: process items one at a time
                when order matters, like applying multiple changes to the same file.
              </li>
              <li>
                <strong>Adversarial review</strong>: implement, then judge, then
                revise. Loop until a critic agent approves the work.
              </li>
            </ul>
          </div>
          <div className="col col--6">
            <ul>
              <li>
                <strong>Error recovery</strong>: post hooks catch failures and
                route them to fix-up agents instead of just retrying blindly.
              </li>
              <li>
                <strong>Hooks</strong>: enrich context before an agent sees it,
                validate results after, clean up resources when a subtree completes.
              </li>
              <li>
                <strong>Schema validation</strong>: each step declares what data
                it accepts. Malformed responses are rejected before they propagate.
              </li>
              <li>
                <strong>Commands</strong>: deterministic shell scripts for the
                mechanical parts: listing files, calling APIs, running builds.
                Save the LLM for the thinking.
              </li>
            </ul>
          </div>
        </div>
        <p className={styles.closingNote}>
          Each pattern is a JSON programme. No framework, no SDK, no
          custom language. Define the state machine, point it at a troupe,
          and let Barnum handle the orchestration.
        </p>
      </div>
    </section>
  );
}

function HowItWorks() {
  return (
    <section className="alt-background">
      <div className="container padding-vert--lg">
        <h2 className={styles.centeredHeading}>Ladies and gentlemen, the show is about to begin!</h2>
        <div className="row">
          <div className="col col--4">
            <h3>📜 1. Write the programme</h3>
            <p>
              Write a programme with steps, transitions, and schemas.
              Each step is either an agent task or a shell command.
            </p>
          </div>
          <div className="col col--4">
            <h3>🎪 2. Corral the troupe</h3>
            <p>
              Start a troupe and connect agents to it. The more
              agents you add, the more work runs in parallel.
            </p>
          </div>
          <div className="col col--4">
            <h3>🎬 3. Showtime</h3>
            <p>
              Hand the programme to Barnum and it distributes tasks
              across your agents, enforces valid transitions, retries
              failures, and respects concurrency limits.
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
      title="Barnum - The ringmaster for your agents"
      description="Don't just /loop it. Barnum is the ringmaster for your agents. Define complex trees of work as statically analyzable state machines."
    >
      <HomepageHeader />
      <main>
        <Features />
        <ExampleSection />
        <AgentAuthoring />
        <WhyBarnum />
        <HowItWorks />
      </main>
    </Layout>
  );
}
