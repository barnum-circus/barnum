import type { SidebarsConfig } from '@docusaurus/plugin-content-docs';

const sidebars: SidebarsConfig = {
  documentationSidebar: [
    'index',
    'quickstart',
    {
      type: 'category',
      label: 'Reference',
      items: [
        'reference/cli',
        'reference/config-schema',
        'reference/task-format',
        'reference/agent-protocol',
        'reference/submission-protocol',
        'reference/retry',
        'reference/resume',
        'reference/visualization',
        'reference/file-extraction',
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
      ],
    },
    {
      type: 'doc',
      id: 'coming-attractions',
      label: 'Coming Attractions',
    },
  ],
};

export default sidebars;
