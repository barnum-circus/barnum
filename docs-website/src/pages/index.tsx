import Layout from '@theme/Layout';
import HomepageHeader from '../components/Header';

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

function WhyGSD() {
  return (
    <section>
      <div className="container padding-vert--lg">
        <h2>Why GSD?</h2>
        <p>
          LLMs are powerful but naive looping falls apart on complex tasks.
          Context fills up, agents forget instructions, and you're left
          debugging a black box. GSD gives you the structure to orchestrate
          ambitious, multi-agent work — fan-out, branching, adversarial
          review, error recovery — with predictable behavior.
        </p>
        <p>
          Define your workflow as a JSON config. Each step declares its valid
          transitions and schemas. The runtime enforces the rules, retries
          failures, and keeps each agent focused on exactly one task. Think
          Buck/Bazel, but the dependency graph is discovered at runtime as
          agents decide what to spawn.
        </p>
      </div>
    </section>
  );
}

export default function Home(): JSX.Element {
  return (
    <Layout
      title="GSD - The build system for LLM agents"
      description="Don't just loop it. GSD is a workflow engine for LLM agents — define complex trees of work as statically analyzable state machines."
    >
      <HomepageHeader />
      <main>
        <Features />
        <WhyGSD />
      </main>
    </Layout>
  );
}
