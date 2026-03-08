import Layout from '@theme/Layout';
import HomepageHeader from '../components/Header';

function Features() {
  return (
    <section className="alt-background">
      <div className="container padding-vert--lg">
        <div className="row">
          <div className="col col--4">
            <h3>Type-Safe State Machines</h3>
            <p>
              Define task queues as type-safe state machines with validated
              transitions. Know exactly what states your agents can be in.
            </p>
          </div>
          <div className="col col--4">
            <h3>Context Protection</h3>
            <p>
              Agents only see the instructions they need for their current task.
              Progressive disclosure keeps context focused and prevents confusion.
            </p>
          </div>
          <div className="col col--4">
            <h3>Long-Lived Agents</h3>
            <p>
              Agents persist across tasks, avoiding startup costs. A pool of
              workers handles tasks as they arrive.
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
          LLMs are powerful but struggle with long, complex tasks. As context
          fills up, they become forgetful and make mistakes. GSD provides
          structure that enables LLMs to perform dramatically more ambitious
          tasks.
        </p>
        <p>
          With GSD, you define a state machine via JSON config. Each step gets
          only the context it needs. Agents can handle increasing complexity
          because they're not overwhelmed with irrelevant information.
        </p>
      </div>
    </section>
  );
}

export default function Home(): JSX.Element {
  return (
    <Layout
      title="GSD - Task Queues for LLM Agents"
      description="GSD is a set of tools for defining task queues as type-safe state machines whose tasks are executed by long-lived agents."
    >
      <HomepageHeader />
      <main>
        <Features />
        <WhyGSD />
      </main>
    </Layout>
  );
}
