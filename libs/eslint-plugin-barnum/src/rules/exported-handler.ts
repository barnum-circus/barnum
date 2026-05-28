import type { Rule } from "eslint";

const HANDLER_FACTORIES = new Set([
  "createHandler",
  "createHandlerWithConfig",
]);

const rule: Rule.RuleModule = {
  meta: {
    type: "problem",
    docs: {
      description:
        "Require createHandler/createHandlerWithConfig calls to be exported with a name matching the exportName argument",
    },
    messages: {
      unexported:
        "createHandler/createHandlerWithConfig must be exported. The runtime resolves handlers by module path + export name.",
      nameMismatch:
        "Export name '{{ exportName }}' does not match the exportName argument '{{ argName }}'. The runtime uses the exportName argument to locate the handler at {{ module }}:{{ argName }}.",
      defaultMissingArg:
        "Default exports should omit the exportName argument or pass 'default'. Got '{{ argName }}'.",
      moduleExports:
        "module.exports assignment is not supported. Use ES module export syntax.",
    },
    schema: [],
  },

  create(context) {
    function getCalleeHandlerName(
      node: { type: string; callee?: { type: string; name?: string } },
    ): string | undefined {
      if (
        node.type === "CallExpression" &&
        node.callee?.type === "Identifier" &&
        node.callee.name &&
        HANDLER_FACTORIES.has(node.callee.name)
      ) {
        return node.callee.name;
      }
      return undefined;
    }

    function getExportNameArg(
      node: Rule.Node & { type: "CallExpression" },
    ): string | null {
      // Second argument is the exportName string
      const arg = node.arguments[1];
      if (!arg) return null;
      if (arg.type === "Literal" && typeof arg.value === "string") {
        return arg.value;
      }
      // Non-string-literal second arg — can't statically verify
      return null;
    }

    return {
      // Catch module.exports.foo = createHandler(...)
      AssignmentExpression(node) {
        if (
          node.right.type === "CallExpression" &&
          getCalleeHandlerName(node.right)
        ) {
          if (
            node.left.type === "MemberExpression" &&
            node.left.object.type === "MemberExpression" &&
            node.left.object.object.type === "Identifier" &&
            node.left.object.object.name === "module" &&
            node.left.object.property.type === "Identifier" &&
            node.left.object.property.name === "exports"
          ) {
            context.report({ node, messageId: "moduleExports" });
            return;
          }
          if (
            node.left.type === "MemberExpression" &&
            node.left.object.type === "Identifier" &&
            node.left.object.name === "exports"
          ) {
            context.report({ node, messageId: "moduleExports" });
            return;
          }
        }
      },

      // Catch all CallExpressions to createHandler/createHandlerWithConfig
      CallExpression(node) {
        if (!getCalleeHandlerName(node)) return;

        // Walk up to find if this is in an export context
        const parent = (node as Rule.Node).parent;
        if (!parent) {
          context.report({ node, messageId: "unexported" });
          return;
        }

        // Already reported by AssignmentExpression handler
        if (parent.type === "AssignmentExpression") {
          return;
        }

        // Case 1: export default createHandler(...)
        if (parent.type === "ExportDefaultDeclaration") {
          const argName = getExportNameArg(
            node as Rule.Node & { type: "CallExpression" },
          );
          if (argName !== null && argName !== "default") {
            context.report({
              node,
              messageId: "defaultMissingArg",
              data: { argName },
            });
          }
          return;
        }

        // Case 2: export const foo = createHandler({...}, "foo")
        // The CallExpression is inside a VariableDeclarator
        if (parent.type === "VariableDeclarator") {
          const declaration = parent.parent;
          if (!declaration || declaration.type !== "VariableDeclaration") {
            context.report({ node, messageId: "unexported" });
            return;
          }
          const exportNode = declaration.parent;
          if (
            !exportNode ||
            exportNode.type !== "ExportNamedDeclaration"
          ) {
            context.report({ node, messageId: "unexported" });
            return;
          }

          // It's exported — now check name match
          if (parent.id.type !== "Identifier") {
            // Destructuring pattern — weird but report
            context.report({ node, messageId: "unexported" });
            return;
          }

          const exportName = parent.id.name;
          const argName = getExportNameArg(
            node as Rule.Node & { type: "CallExpression" },
          );

          // If no second arg, the runtime uses "default" — that won't match
          // a named export unless the export is literally named "default"
          if (argName === null && node.arguments.length < 2) {
            // No exportName arg provided — runtime defaults to "default"
            // but this is a named export, so it won't resolve
            if (exportName !== "default") {
              context.report({
                node,
                messageId: "nameMismatch",
                data: {
                  exportName,
                  argName: "default",
                  module: "<this file>",
                },
              });
            }
            return;
          }

          if (argName !== null && argName !== exportName) {
            context.report({
              node,
              messageId: "nameMismatch",
              data: {
                exportName,
                argName,
                module: "<this file>",
              },
            });
          }
          return;
        }

        // Any other position — not exported
        context.report({ node, messageId: "unexported" });
      },
    };
  },
};

export default rule;
