import { describe, it, expect } from "vitest";
import { z } from "zod";
import { zodToCheckedJsonSchema } from "../src/schema.js";

const convert = (schema: z.ZodType) => zodToCheckedJsonSchema(schema, "test");

describe("zodToCheckedJsonSchema", () => {
  describe("primitives", () => {
    it("converts z.string()", () => {
      expect(convert(z.string())).toEqual({ type: "string" });
    });

    it("converts z.number()", () => {
      expect(convert(z.number())).toEqual({ type: "number" });
    });

    it("converts z.boolean()", () => {
      expect(convert(z.boolean())).toEqual({ type: "boolean" });
    });

    it("converts z.null()", () => {
      expect(convert(z.null())).toEqual({ type: "null" });
    });

    it("converts z.unknown()", () => {
      expect(convert(z.unknown())).toEqual({});
    });

    it("converts z.any()", () => {
      expect(convert(z.any())).toEqual({});
    });
  });

  describe("literals", () => {
    it("converts string literal", () => {
      expect(convert(z.literal("hello"))).toEqual({
        type: "string",
        const: "hello",
      });
    });

    it("converts number literal", () => {
      expect(convert(z.literal(42))).toEqual({
        type: "number",
        const: 42,
      });
    });

    it("converts boolean literal", () => {
      expect(convert(z.literal(true))).toEqual({
        type: "boolean",
        const: true,
      });
    });

    it("converts null literal", () => {
      expect(convert(z.literal(null))).toEqual({
        type: "null",
        const: null,
      });
    });
  });

  describe("enum", () => {
    it("converts z.enum()", () => {
      expect(convert(z.enum(["a", "b", "c"]))).toEqual({
        type: "string",
        enum: ["a", "b", "c"],
      });
    });
  });

  describe("containers", () => {
    it("converts z.object()", () => {
      expect(convert(z.object({ a: z.string(), b: z.number() }))).toEqual({
        type: "object",
        properties: {
          a: { type: "string" },
          b: { type: "number" },
        },
        required: ["a", "b"],
        additionalProperties: false,
      });
    });

    it("converts z.object() with optional field", () => {
      expect(
        convert(z.object({ a: z.string(), b: z.number().optional() })),
      ).toEqual({
        type: "object",
        properties: {
          a: { type: "string" },
          b: { type: "number" },
        },
        required: ["a"],
        additionalProperties: false,
      });
    });

    it("converts z.array()", () => {
      expect(convert(z.array(z.number()))).toEqual({
        type: "array",
        items: { type: "number" },
      });
    });

    it("converts z.tuple()", () => {
      expect(convert(z.tuple([z.string(), z.number()]))).toEqual({
        type: "array",
        items: [{ type: "string" }, { type: "number" }],
      });
    });

    it("converts z.record()", () => {
      expect(convert(z.record(z.string(), z.number()))).toEqual({
        type: "object",
        propertyNames: { type: "string" },
        additionalProperties: { type: "number" },
      });
    });
  });

  describe("composition", () => {
    it("converts z.union()", () => {
      expect(convert(z.union([z.string(), z.number()]))).toEqual({
        anyOf: [{ type: "string" }, { type: "number" }],
      });
    });

    it("converts z.nullable()", () => {
      expect(convert(z.nullable(z.string()))).toEqual({
        anyOf: [{ type: "string" }, { type: "null" }],
      });
    });
  });

  describe("string modifiers", () => {
    it("min length", () => {
      expect(convert(z.string().min(3))).toEqual({
        type: "string",
        minLength: 3,
      });
    });

    it("max length", () => {
      expect(convert(z.string().max(10))).toEqual({
        type: "string",
        maxLength: 10,
      });
    });

    it("exact length", () => {
      expect(convert(z.string().length(5))).toEqual({
        type: "string",
        minLength: 5,
        maxLength: 5,
      });
    });

    it("regex", () => {
      expect(convert(z.string().regex(/^foo/))).toEqual({
        type: "string",
        pattern: "^foo",
      });
    });

    it("email format", () => {
      const result = convert(z.string().email());
      expect(result).toMatchObject({
        type: "string",
        format: "email",
      });
      // Zod v4 also emits a pattern for email validation
      expect(result).toHaveProperty("pattern");
    });

    it("url format", () => {
      expect(convert(z.string().url())).toEqual({
        type: "string",
        format: "uri",
      });
    });

    it("startsWith", () => {
      expect(convert(z.string().startsWith("foo"))).toEqual({
        type: "string",
        pattern: "^foo.*",
      });
    });

    it("endsWith", () => {
      expect(convert(z.string().endsWith("bar"))).toEqual({
        type: "string",
        pattern: ".*bar$",
      });
    });
  });

  describe("number modifiers", () => {
    it("minimum", () => {
      expect(convert(z.number().min(0))).toEqual({
        type: "number",
        minimum: 0,
      });
    });

    it("maximum", () => {
      expect(convert(z.number().max(100))).toEqual({
        type: "number",
        maximum: 100,
      });
    });

    it("exclusive minimum", () => {
      expect(convert(z.number().gt(0))).toEqual({
        type: "number",
        exclusiveMinimum: 0,
      });
    });

    it("exclusive maximum", () => {
      expect(convert(z.number().lt(100))).toEqual({
        type: "number",
        exclusiveMaximum: 100,
      });
    });

    it("integer", () => {
      expect(convert(z.number().int())).toEqual({
        type: "integer",
        minimum: -9007199254740991,
        maximum: 9007199254740991,
      });
    });

    it("multipleOf", () => {
      expect(convert(z.number().multipleOf(5))).toEqual({
        type: "number",
        multipleOf: 5,
      });
    });
  });

  describe("array modifiers", () => {
    it("minItems", () => {
      expect(convert(z.array(z.string()).min(1))).toEqual({
        type: "array",
        items: { type: "string" },
        minItems: 1,
      });
    });

    it("maxItems", () => {
      expect(convert(z.array(z.string()).max(10))).toEqual({
        type: "array",
        items: { type: "string" },
        maxItems: 10,
      });
    });
  });

  describe("transparent wrappers", () => {
    it(".default() adds default value", () => {
      expect(convert(z.string().default("hello"))).toEqual({
        type: "string",
        default: "hello",
      });
    });

    it("standalone .optional() produces inner type", () => {
      expect(convert(z.string().optional())).toEqual({
        type: "string",
      });
    });
  });

  describe("rejected types", () => {
    it("throws for z.undefined()", () => {
      expect(() => convert(z.undefined())).toThrow(
        /Undefined cannot be represented in JSON Schema/,
      );
    });

    it("throws for z.void()", () => {
      expect(() => convert(z.void())).toThrow(
        /Void cannot be represented in JSON Schema/,
      );
    });

    it("throws for z.bigint()", () => {
      expect(() => convert(z.bigint())).toThrow(
        /BigInt cannot be represented in JSON Schema/,
      );
    });

    it("throws for z.symbol()", () => {
      expect(() => convert(z.symbol())).toThrow(
        /Symbols cannot be represented in JSON Schema/,
      );
    });

    it("throws for z.date()", () => {
      expect(() => convert(z.date())).toThrow(
        /Date cannot be represented in JSON Schema/,
      );
    });

    it("throws for z.function()", () => {
      expect(() => convert(z.function())).toThrow(
        /Function types cannot be represented in JSON Schema/,
      );
    });

    it("throws for z.map()", () => {
      expect(() => convert(z.map(z.string(), z.number()))).toThrow(
        /Map cannot be represented in JSON Schema/,
      );
    });

    it("throws for z.set()", () => {
      expect(() => convert(z.set(z.string()))).toThrow(
        /Set cannot be represented in JSON Schema/,
      );
    });

    it("throws for .transform()", () => {
      expect(() =>
        convert(z.string().transform((s) => parseInt(s, 10))),
      ).toThrow(/Transforms cannot be represented in JSON Schema/);
    });
  });

  describe("nested rejected types", () => {
    it("throws for rejected type inside object", () => {
      expect(() => convert(z.object({ a: z.function() }))).toThrow(
        /Function types cannot be represented/,
      );
    });

    it("throws for rejected type inside array", () => {
      expect(() => convert(z.array(z.set(z.string())))).toThrow(
        /Set cannot be represented/,
      );
    });

    it("throws for rejected type inside union", () => {
      expect(() => convert(z.union([z.string(), z.undefined()]))).toThrow(
        /Undefined cannot be represented/,
      );
    });
  });

  describe("pre-validation rejections", () => {
    it("throws for z.intersection()", () => {
      expect(() =>
        convert(
          z.intersection(
            z.object({ a: z.string() }),
            z.object({ b: z.number() }),
          ),
        ),
      ).toThrow(/z\.intersection\(\) is not supported/);
    });

    it("throws for .refine()", () => {
      expect(() =>
        convert(z.string().refine((s) => s.length > 0)),
      ).toThrow(/\.refine\(\) and \.superRefine\(\) are not supported/);
    });

    it("throws for .superRefine()", () => {
      expect(() => convert(z.string().superRefine(() => {}))).toThrow(
        /\.refine\(\) and \.superRefine\(\) are not supported/,
      );
    });

    it("throws for .refine() on an object", () => {
      expect(() =>
        convert(
          z.object({ a: z.string() }).refine((o) => o.a.length > 0),
        ),
      ).toThrow(/\.refine\(\) and \.superRefine\(\) are not supported/);
    });

    it("throws for .refine() nested inside an object value", () => {
      expect(() =>
        convert(z.object({ a: z.string().refine((s) => s.length > 0) })),
      ).toThrow(/\.refine\(\) and \.superRefine\(\) are not supported/);
    });

    it("throws for .refine() nested inside an array", () => {
      expect(() =>
        convert(z.array(z.string().refine((s) => s.length > 0))),
      ).toThrow(/\.refine\(\) and \.superRefine\(\) are not supported/);
    });

    it("throws for intersection nested inside a union", () => {
      expect(() =>
        convert(
          z.union([
            z.string(),
            z.intersection(
              z.object({ a: z.string() }),
              z.object({ b: z.number() }),
            ),
          ]),
        ),
      ).toThrow(/z\.intersection\(\) is not supported/);
    });

    it("allows built-in checks like .min() alongside rejection of .refine()", () => {
      // .min() is a built-in check, not custom — should not be rejected
      expect(convert(z.string().min(3))).toEqual({
        type: "string",
        minLength: 3,
      });
    });
  });

  describe("error messages", () => {
    it("wraps the error with the handler label", () => {
      expect(() =>
        zodToCheckedJsonSchema(z.undefined(), "myHandler:input"),
      ).toThrow(
        'Handler "myHandler:input": Zod schema cannot be converted to JSON Schema: Undefined cannot be represented in JSON Schema',
      );
    });
  });

  describe("output format", () => {
    it("does not include $schema property", () => {
      const result = convert(z.string());
      expect(result).not.toHaveProperty("$schema");
    });
  });

  describe("domain patterns", () => {
    it("tagged union (HasErrors/Clean)", () => {
      const TypeErrorValidator = z.object({
        file: z.string(),
        message: z.string(),
      });
      const schema = z.union([
        z.object({
          kind: z.literal("HasErrors"),
          value: z.array(TypeErrorValidator),
        }),
        z.object({
          kind: z.literal("Clean"),
          value: z.null(),
        }),
      ]);
      const result = convert(schema);
      expect(result.anyOf).toHaveLength(2);
      expect((result.anyOf as Record<string, unknown>[])[0]).toEqual({
        type: "object",
        properties: {
          kind: { type: "string", const: "HasErrors" },
          value: {
            type: "array",
            items: {
              type: "object",
              properties: {
                file: { type: "string" },
                message: { type: "string" },
              },
              required: ["file", "message"],
              additionalProperties: false,
            },
          },
        },
        required: ["kind", "value"],
        additionalProperties: false,
      });
      expect((result.anyOf as Record<string, unknown>[])[1]).toEqual({
        type: "object",
        properties: {
          kind: { type: "string", const: "Clean" },
          value: { type: "null" },
        },
        required: ["kind", "value"],
        additionalProperties: false,
      });
    });

    it("Result<string, string>", () => {
      const schema = z.union([
        z.object({ kind: z.literal("Ok"), value: z.string() }),
        z.object({ kind: z.literal("Err"), value: z.string() }),
      ]);
      const result = convert(schema);
      expect(result.anyOf).toHaveLength(2);
    });

    it("JudgmentResult (heterogeneous union)", () => {
      const schema = z.union([
        z.object({ approved: z.literal(true) }),
        z.object({
          approved: z.literal(false),
          instructions: z.string(),
        }),
      ]);
      const result = convert(schema);
      expect(result.anyOf).toHaveLength(2);
      // First variant: { approved: true }
      expect((result.anyOf as Record<string, unknown>[])[0]).toEqual({
        type: "object",
        properties: {
          approved: { type: "boolean", const: true },
        },
        required: ["approved"],
        additionalProperties: false,
      });
      // Second variant: { approved: false, instructions: string }
      expect((result.anyOf as Record<string, unknown>[])[1]).toEqual({
        type: "object",
        properties: {
          approved: { type: "boolean", const: false },
          instructions: { type: "string" },
        },
        required: ["approved", "instructions"],
        additionalProperties: false,
      });
    });
  });
});
