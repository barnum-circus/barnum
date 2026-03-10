import type { SidebarsConfig } from '@docusaurus/plugin-content-docs';

const sidebars: SidebarsConfig = {
  documentationSidebar: [
    'index',
    'quickstart',
    {
      type: 'category',
      label: 'Recipes',
      items: [
        'recipes/index',
        'recipes/linear-pipeline',
        'recipes/branching',
        'recipes/branching-refactor',
        'recipes/fan-out',
        'recipes/fan-out-finally',
        'recipes/sequential',
        'recipes/adversarial-review',
        'recipes/error-recovery',
        'recipes/hooks',
        'recipes/validation',
        'recipes/commands',
        'recipes/code-review',
      ],
    },
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
      ],
    },
    'roadmap',
  ],
};

export default sidebars;
