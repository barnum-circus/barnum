import Link from '@docusaurus/Link';
import styles from './styles.module.css';

export default function Buttons() {
  return (
    <div className={styles.buttons}>
      <Link
        className="button button--secondary button--lg"
        to="/docs/quickstart"
      >
        Get started
      </Link>
      <Link
        className="button button--secondary button--lg"
        to="https://github.com/barnum-circus/barnum"
      >
        GitHub
      </Link>
    </div>
  );
}
