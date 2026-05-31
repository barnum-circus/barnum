import type { Rule } from "eslint";

/**
 * Combinator name → minimum required callback params (first argument).
 * Applies to both standalone calls and postfix method calls by name.
 * Add/remove entries here to adjust what's tracked.
 */
const MIN_CALLBACK_PARAMS = new Map<string, number>([
  ["bindInput", 1],
  ["loop", 1],
  ["earlyReturn", 1],
]);

const rule: Rule.RuleModule = {
  meta: {
    type: "problem",
    docs: {
      description:
        "Require combinator callbacks to declare their parameters. A zero-arity callback ignores values the combinator provides.",
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

        if (node.callee.type === "Identifier") {
          name = (node.callee as { name: string }).name;
        } else if (
          node.callee.type === "MemberExpression" &&
          (node.callee as { property: { type: string; name?: string } })
            .property.type === "Identifier"
        ) {
          name = (node.callee as { property: { name: string } }).property.name;
        } else {
          return;
        }

        const minParams = MIN_CALLBACK_PARAMS.get(name);
        if (minParams === undefined) return;

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
