import clsx from 'clsx';
import Buttons from '../Buttons';
import styles from './styles.module.css';

export default function HomepageHeader() {
  return (
    <header className={clsx('hero hero--primary', styles.heroBanner)}>
      <div className="container">
        <div className="row">
          <div className="col">
            <h1 className={styles.heroTitle}>Barnum</h1>
            <p
              className={clsx(
                'hero__subtitle margin-bottom--lg',
                styles.heroSubtitle,
              )}
            >
              The programming language for orchestrating agents.
            </p>
            <Buttons />
          </div>
        </div>
      </div>
    </header>
  );
}
