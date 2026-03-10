import Link from '@docusaurus/Link';
import styles from './styles.module.css';

export default function Buttons() {
  return (
    <div className={styles.buttons}>
      <Link
        className="button button--secondary button--lg"
        to="/docs/quickstart"
      >
        Quick Start
      </Link>
      <Link
        className="button button--secondary button--lg"
        to="https://github.com/barnum-circus/barnum"
      >
        View on GitHub
      </Link>
    </div>
  );
}
