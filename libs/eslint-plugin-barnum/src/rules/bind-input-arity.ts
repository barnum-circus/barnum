import type { Rule } from "eslint";

const rule: Rule.RuleModule = {
  meta: {
    type: "problem",
    docs: {
      description:
        "Require bindInput callbacks to declare exactly one parameter. A zero-arity callback ignores the captured input — use `bind` or inline the action instead.",
    },
    messages: {
      missingParam:
        "'bindInput' callback must declare a parameter for the captured input. If you don't need the input, use a different combinator.",
    },
    schema: [],
  },

  create(context) {
    function checkBindInput(node: Rule.Node & { type: "CallExpression" }) {
      // Get the first argument (the body callback)
      const bodyArg = (node as any).arguments?.[0];
      if (!bodyArg) return;

      if (
        bodyArg.type === "ArrowFunctionExpression" ||
        bodyArg.type === "FunctionExpression"
      ) {
        if (bodyArg.params.length === 0) {
          context.report({
            node: bodyArg,
            messageId: "missingParam",
          });
        }
      }
    }

    return {
      CallExpression(node) {
        // Standalone: bindInput(...)
        if (node.callee.type === "Identifier") {
          const name = (node.callee as { name: string }).name;
          if (name === "bindInput") {
            checkBindInput(node as Rule.Node & { type: "CallExpression" });
          }
        }
        // Postfix: x.bindInput(...)
        else if (
          node.callee.type === "MemberExpression" &&
          (node.callee as { property: { type: string; name?: string } })
            .property.type === "Identifier" &&
          (node.callee as { property: { name: string } }).property.name ===
            "bindInput"
        ) {
          checkBindInput(node as Rule.Node & { type: "CallExpression" });
        }
      },
    };
  },
};

export default rule;
