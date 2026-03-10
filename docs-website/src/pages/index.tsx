import CodeBlock from '@theme/CodeBlock';
import Layout from '@theme/Layout';
import HomepageHeader from '../components/Header';

const exampleConfig = `{
  "entrypoint": "ListFiles",
  "steps": [
    {
      "name": "ListFiles",
      "action": {
        "kind": "Command",
        "script": "find src -name '*.rs' | jq -R -s 'split(\"\\n\") | map(select(. != \"\")) | map({kind: \"Refactor\", value: {file: .}})'"
      },
      "next": ["Refactor"]
    },
    {
      "name": "Refactor",
      "value_schema": {
        "type": "object",
        "required": ["file"],
        "properties": {
          "file": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": {
          "inline": "Refactor this file for clarity. Return \`[]\`."
        }
      },
      "next": [],
      "finally": "echo '[]'"
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
        <div className="row">
          <div className="col col--6">
            <h2>One config. Complex workflows.</h2>
            <p>
              A command lists files. Each file fans out to a parallel
              refactoring agent. A <code>finally</code> hook runs when
              all agents finish. The entire workflow is a single JSON
              file — no imperative glue code.
            </p>
            <CodeBlock language="json" title="config.jsonc">
              {exampleConfig}
            </CodeBlock>
          </div>
          <div className="col col--6" style={{display: 'flex', alignItems: 'center', justifyContent: 'center'}}>
            <div style={{
              border: '2px dashed var(--ifm-color-primary-light)',
              borderRadius: '12px',
              padding: '3rem 2rem',
              textAlign: 'center',
              width: '100%',
              color: 'var(--ifm-color-primary-dark)',
            }}>
              <p style={{fontSize: '1.1rem', marginBottom: '0.5rem', fontWeight: 600}}>
                asciinema demo coming soon
              </p>
              <p style={{fontSize: '0.9rem', margin: 0, opacity: 0.7}}>
                Watch GSD orchestrate a multi-file refactor in real time
              </p>
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}

function WhyGSD() {
  return (
    <section className="alt-background">
      <div className="container padding-vert--lg">
        <h2>Why GSD? <span style={{fontWeight: 400, fontSize: '0.6em', opacity: 0.7}}>(Get Sh*** Done)</span></h2>
        <p>
          Looping tools are great for simple tasks: keep trying until it
          works. But they hit a wall fast. Context fills up, the agent forgets
          what it already did, and you can't reason about what will happen
          before you run it. For anything beyond a single-file fix, you need
          actual structure.
        </p>
        <h3>What GSD gives you</h3>
        <div className="row" style={{marginTop: '1rem'}}>
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
        <p style={{marginTop: '1rem'}}>
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
      title="GSD - The missing workflow engine for LLM agents"
      description="Don't just /loop it. GSD is the missing workflow engine for LLM agents — define complex trees of work as statically analyzable state machines."
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
