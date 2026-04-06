import type { SidebarsConfig } from '@docusaurus/plugin-content-docs';

const sidebars: SidebarsConfig = {
  documentationSidebar: [
    'index',
    'quickstart',
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
        'architecture/compiler',
        'architecture/algebraic-effect-handlers',
        'architecture/validation',
      ],
    },
    {
      type: 'category',
      label: 'Repertoire',
      items: [
        'repertoire/index',
        'repertoire/linear-pipeline',
        'repertoire/branching',
        'repertoire/branching-refactor',
        'repertoire/fan-out',
        'repertoire/fan-out-finally',
        'repertoire/sequential',
        'repertoire/adversarial-review',
        'repertoire/error-recovery',
        'repertoire/hooks',
        'repertoire/validation',
        'repertoire/commands',
        'repertoire/code-review',
        'repertoire/legal-review',
        'repertoire/document-verification',
        'repertoire/editing-assistant',
      ],
    },
  ],
};

export default sidebars;
