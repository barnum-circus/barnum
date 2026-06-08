import type { Rule } from "eslint";

/**
 * Configuration for each combinator:
 * - minParams: minimum required type parameters
 * - params: array describing each position — "input" or "output"
 *   - "input" positions disallow `unknown`
 *   - "output" positions disallow `any`
 */
interface CombinatorConfig {
  minParams: number;
  params: Array<"input" | "output">;
}

// Only combinators whose output silently degrades to `any` when type params are
// omitted need this rule. `loop<TIn = void, TOut = void>` defaults to a concrete
// output (and still type-checks its body), so it's intentionally absent here.
// `earlyReturn<TEarlyReturn, TIn = any, TOut = any>` degrades both TIn and TOut
// unless all three are supplied — hence minParams 3, not 1.
/** Standalone call configs: bindInput(...), earlyReturn(...) */
const STANDALONE_CONFIG = new Map<string, CombinatorConfig>([
  ["earlyReturn", { minParams: 3, params: ["output", "input", "output"] }], // TEarlyReturn, TIn, TOut
  ["bindInput", { minParams: 2, params: ["input", "output"] }], // TIn, TOut
]);

/** Postfix method call configs: x.bindInput<TOut>(...) */
const POSTFIX_CONFIG = new Map<string, CombinatorConfig>([
  ["bindInput", { minParams: 1, params: ["output"] }], // TOut only
]);

function getTypeName(node: { type: string; typeName?: { name?: string } }): string | null {
  if (node.type === "TSAnyKeyword") return "any";
  if (node.type === "TSUnknownKeyword") return "unknown";
  if (node.type === "TSNeverKeyword") return "never";
  if (node.type === "TSTypeReference" && node.typeName?.name) {
    return node.typeName.name;
  }
  return null;
}

const rule: Rule.RuleModule = {
  meta: {
    type: "problem",
    docs: {
      description:
        "Require explicit type parameters on earlyReturn and bindInput (combinators whose output degrades to `any` without them). Disallow `any` in output positions and `unknown` in input positions.",
    },
    messages: {
      missingTypeParams:
        "'{{ name }}' requires at least {{ min }} explicit type parameter(s). Omitting them defaults the input/output to `any`, silently disabling type checking downstream.",
      anyInOutput:
        "Type parameter {{ position }} of '{{ name }}' is an output type — `any` defeats type checking. Use a concrete type or `never` for early-return bodies.",
      unknownInInput:
        "Type parameter {{ position }} of '{{ name }}' is an input type — `unknown` is too wide. Use the concrete input type.",
    },
    schema: [],
  },

  create(context) {
    return {
      CallExpression(node) {
        let name: string;
        let config: CombinatorConfig | undefined;

        if (node.callee.type === "Identifier") {
          name = (node.callee as { name: string }).name;
          config = STANDALONE_CONFIG.get(name);
        } else if (
          node.callee.type === "MemberExpression" &&
          (node.callee as { property: { type: string; name?: string } }).property.type === "Identifier"
        ) {
          name = (node.callee as { property: { name: string } }).property.name;
          config = POSTFIX_CONFIG.get(name);
        } else {
          return;
        }

        if (!config) return;

        // Check for type parameters (TSTypeParameterInstantiation)
        // typescript-eslint uses "typeArguments" on CallExpression nodes
        const typeParams = (node as unknown as { typeArguments?: { params: Array<{ type: string; typeName?: { name?: string } }> } }).typeArguments;

        if (!typeParams || typeParams.params.length < config.minParams) {
          context.report({
            node,
            messageId: "missingTypeParams",
            data: { name, min: String(config.minParams) },
          });
          return;
        }

        // Check each provided type parameter
        for (let i = 0; i < typeParams.params.length; i++) {
          const param = typeParams.params[i];
          const position = config.params[i];
          if (!position) break;

          const typeName = getTypeName(param);

          if (position === "output" && typeName === "any") {
            context.report({
              node: param as unknown as Rule.Node,
              messageId: "anyInOutput",
              data: { name, position: String(i + 1) },
            });
          }

          if (position === "input" && typeName === "unknown") {
            context.report({
              node: param as unknown as Rule.Node,
              messageId: "unknownInInput",
              data: { name, position: String(i + 1) },
            });
          }
        }
      },
    };
  },
};

export default rule;
