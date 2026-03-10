import clsx from 'clsx';
import Buttons from '../Buttons';
import styles from './styles.module.css';

export default function HomepageHeader() {
  return (
    <header className={clsx('hero hero--primary', styles.heroBanner)}>
      <div className="container">
        <div className="row">
          <div className="col">
            <h1 className={styles.heroTitle}>GSD</h1>
            <p
              className={clsx(
                'hero__subtitle margin-bottom--lg',
                styles.heroSubtitle,
              )}
            >
              Don't just /loop it.
              <br />
              The missing workflow engine for LLM agents.
            </p>
            <Buttons />
          </div>
        </div>
      </div>
    </header>
  );
}
