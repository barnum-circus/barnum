import type { Rule } from "eslint";

/**
 * Configuration: combinator name → minimum required callback params.
 */
const STANDALONE_CONFIG = new Map<string, number>([
  ["bindInput", 1],
  ["loop", 1],
  ["earlyReturn", 1],
]);

const POSTFIX_CONFIG = new Map<string, number>([
  ["bindInput", 1],
]);

const rule: Rule.RuleModule = {
  meta: {
    type: "problem",
    docs: {
      description:
        "Require combinator callbacks to declare their parameters. A zero-arity callback to bindInput, loop, or earlyReturn ignores values the combinator provides.",
    },
    messages: {
      missingParams:
        "'{{ name }}' callback must declare at least {{ min }} parameter(s). The callback receives values from the combinator — ignoring them is likely a bug.",
    },
    schema: [],
  },

  create(context) {
    return {
      CallExpression(node) {
        let name: string;
        let minParams: number | undefined;

        if (node.callee.type === "Identifier") {
          name = (node.callee as { name: string }).name;
          minParams = STANDALONE_CONFIG.get(name);
        } else if (
          node.callee.type === "MemberExpression" &&
          (node.callee as { property: { type: string; name?: string } })
            .property.type === "Identifier"
        ) {
          name = (node.callee as { property: { name: string } }).property.name;
          minParams = POSTFIX_CONFIG.get(name);
        } else {
          return;
        }

        if (minParams === undefined) return;

        // Find the callback argument (first argument for all tracked combinators)
        const bodyArg = (node as any).arguments?.[0];
        if (!bodyArg) return;

        if (
          bodyArg.type === "ArrowFunctionExpression" ||
          bodyArg.type === "FunctionExpression"
        ) {
          if (bodyArg.params.length < minParams) {
            context.report({
              node: bodyArg,
              messageId: "missingParams",
              data: { name, min: String(minParams) },
            });
          }
        }
      },
    };
  },
};

export default rule;
