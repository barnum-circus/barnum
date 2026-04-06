import type { SidebarsConfig } from '@docusaurus/plugin-content-docs';

const sidebars: SidebarsConfig = {
  documentationSidebar: [
    'index',
    'quickstart',
    {
      type: 'category',
      label: 'Patterns',
      items: [
        'patterns/index',
        'patterns/serial-execution',
        'patterns/parallel-execution',
        'patterns/branching',
        'patterns/looping',
        'patterns/error-handling',
        'patterns/timeout',
        'patterns/racing',
        'patterns/context-and-variables',
        'patterns/early-return',
        'patterns/recursion',
      ],
    },
    {
      type: 'category',
      label: 'Repertoire',
      items: [
        'repertoire/index',
        'repertoire/adversarial-review',
        'repertoire/identify-and-refactor',
        'repertoire/code-review',
        'repertoire/document-verification',
        'repertoire/legal-review',
        'repertoire/babysitting-prs',
        'repertoire/codebase-migration',
        'repertoire/dependency-updates',
        'repertoire/test-generation',
        'repertoire/security-remediation',
        'repertoire/release-management',
        'repertoire/incident-triage',
        'repertoire/onboarding-automation',
        'repertoire/api-contract-verification',
        'repertoire/localization',
      ],
    },
    {
      type: 'category',
      label: 'Reference',
      items: [
        'reference/builtins',
        'reference/cli',
      ],
    },
    {
      type: 'category',
      label: 'Architecture',
      items: [
        'architecture/index',
        'architecture/typescript-ast',
        'architecture/postfix-methods',
        'architecture/compiler',
        'architecture/algebraic-effect-handlers',
        'architecture/validation',
      ],
    },
  ],
};

export default sidebars;
