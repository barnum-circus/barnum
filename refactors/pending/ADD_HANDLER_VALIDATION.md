# Add Handler Validation

**Parent:** TS_CONFIG.md
**Depends on:** ADD_RUN_HANDLER

## Motivation

TypeScript handlers declare Zod schemas for their inputs. These schemas serve two purposes:
1. Type safety for handler authors (TypeScript inference from Zod)
2. Runtime validation of step outputs by Rust (via JSON Schema)

Validation does **not** happen in `run-handler.ts`. The handler subprocess just calls the function — it assumes all inputs are valid. Instead:
- **JS validates step config** at `run()` time (before the Rust binary starts)
- **JS converts Zod → JSON Schema** and embeds it in the serialized config
- **Rust validates step outputs** at runtime using the embedded JSON Schema

## Handler interface

A handler exports an object with three parts:

```typescript
import { z } from "zod";
import type { HandlerDefinition } from "@barnum/barnum";

const stepConfigValidator = z.object({
  instructions: z.string(),
});

type StepConfig = z.infer<typeof stepConfigValidator>;

const stepValueValidator = z.object({ file: z.string() });

type StepValue = z.infer<typeof stepValueValidator>;

export default {
  stepConfigValidator,

  getStepValueValidator(_stepConfig) {
    return stepValueValidator;
  },

  async handle({ stepConfig, value, config, stepName }) {
    return [{ kind: "Implement", value: { plan: "..." } }];
  },
} satisfies HandlerDefinition<StepConfig, StepValue>;
```

- **`stepConfigValidator`**: Zod schema for `stepConfig`. Validated in JS at `run()` time.
- **`getStepValueValidator(stepConfig)`**: Returns a Zod schema for the step's input value. Called at `run()` time after step config is validated. The returned schema is converted to JSON Schema and sent to Rust.
- **`handle`**: Called at runtime. Receives validated inputs, returns follow-up tasks.

## TypeScript types

**File:** `libs/barnum/types.ts` (new, exported from package)

```typescript
import type { z } from "zod";

export interface HandlerDefinition<C = unknown, V = unknown> {
  stepConfigValidator: z.ZodType<C>;
  getStepValueValidator: (stepConfig: C) => z.ZodType<V>;
  handle: (context: HandlerContext<C, V>) => Promise<FollowUpTask[]>;
}

export interface HandlerContext<C, V> {
  stepConfig: C;
  value: V;
  config: unknown;
  stepName: string;
}

export interface FollowUpTask {
  kind: string;
  value: unknown;
}
```

## JS-side: config construction (`run.ts`)

### Before (current `run()` method)

```typescript
// run.ts — current
run(opts?: RunOptions): ChildProcess {
  const args = opts?.resumeFrom
    ? ["run", "--resume-from", opts.resumeFrom]
    : ["run", "--config", JSON.stringify(this.config)];
  if (opts?.entrypointValue) args.push("--entrypoint-value", opts.entrypointValue);
  if (opts?.logLevel) args.push("--log-level", opts.logLevel);
  if (opts?.logFile) args.push("--log-file", opts.logFile);
  if (opts?.stateLog) args.push("--state-log", opts.stateLog);
  if (opts?.wake) args.push("--wake", opts.wake);
  args.push("--executor", resolveExecutor());
  args.push("--run-handler-path", runHandlerPath);
  return spawnBarnum(args, opts?.cwd);
}
```

### After

```typescript
// run.ts — after
async run(opts?: RunOptions): Promise<ChildProcess> {
  const config = await this.resolveConfig();
  const args = opts?.resumeFrom
    ? ["run", "--resume-from", opts.resumeFrom]
    : ["run", "--config", JSON.stringify(config)];
  if (opts?.entrypointValue) args.push("--entrypoint-value", opts.entrypointValue);
  if (opts?.logLevel) args.push("--log-level", opts.logLevel);
  if (opts?.logFile) args.push("--log-file", opts.logFile);
  if (opts?.stateLog) args.push("--state-log", opts.stateLog);
  if (opts?.wake) args.push("--wake", opts.wake);
  args.push("--executor", resolveExecutor());
  args.push("--run-handler-path", runHandlerPath);
  return spawnBarnum(args, opts?.cwd);
}
```

`run()` becomes `async` because `resolveConfig()` imports handler modules.

### `resolveConfig()` implementation

```typescript
// run.ts — new private method on BarnumConfig
private async resolveConfig(): Promise<z.output<typeof configSchema>> {
  const config = structuredClone(this.config);

  for (const step of config.steps) {
    if (step.action.kind !== "TypeScript") continue;
    const action = step.action;

    // 1. Import the handler module
    const mod = await import(action.path);
    const handler = mod[action.exportedAs ?? "default"];

    if (!handler?.stepConfigValidator || !handler?.getStepValueValidator) {
      throw new Error(
        `Step "${step.name}": handler at "${action.path}" is missing required ` +
        `"stepConfigValidator" or "getStepValueValidator". ` +
        `See HandlerDefinition interface.`
      );
    }

    // 2. Validate step config
    const parsedStepConfig = handler.stepConfigValidator.parse(action.stepConfig ?? {});

    // 3. Get value validator
    const valueValidator = handler.getStepValueValidator(parsedStepConfig);

    // 4. Reject non-serializable Zod features
    assertSerializableZod(valueValidator, step.name);

    // 5. Convert Zod → JSON Schema
    const jsonSchema = zodToJsonSchema(valueValidator, { target: "jsonSchema7" });

    // 6. Embed in config
    action.valueSchema = jsonSchema;
  }

  return config;
}
```

### `assertSerializableZod()` implementation

```typescript
// run.ts — new helper
import { z } from "zod";

const UNSUPPORTED_ZOD_TYPES = new Set([
  "ZodEffects",     // .transform(), .refine(), .superRefine(), .preprocess()
  "ZodPipeline",    // .pipe()
  "ZodBranded",     // .brand()
]);

function assertSerializableZod(schema: z.ZodType, stepName: string): void {
  const typeName = (schema as any)._def?.typeName;

  if (UNSUPPORTED_ZOD_TYPES.has(typeName)) {
    throw new Error(
      `Step "${stepName}": Zod schema uses unsupported type "${typeName}". ` +
      `Only JSON-Schema-representable types are allowed. ` +
      `Remove .transform(), .refine(), .preprocess(), .pipe(), or .brand().`
    );
  }

  // Recurse into compound types
  const def = (schema as any)._def;
  if (!def) return;

  if (def.innerType) assertSerializableZod(def.innerType, stepName);
  if (def.schema) assertSerializableZod(def.schema, stepName);
  if (def.left) assertSerializableZod(def.left, stepName);
  if (def.right) assertSerializableZod(def.right, stepName);

  // z.object() — check each value
  if (def.shape) {
    for (const value of Object.values(def.shape())) {
      assertSerializableZod(value as z.ZodType, stepName);
    }
  }

  // z.array(), z.set()
  if (def.type) assertSerializableZod(def.type, stepName);

  // z.union(), z.discriminatedUnion()
  if (def.options) {
    for (const option of def.options) {
      assertSerializableZod(option as z.ZodType, stepName);
    }
  }

  // z.tuple()
  if (def.items) {
    for (const item of def.items) {
      assertSerializableZod(item as z.ZodType, stepName);
    }
  }

  // z.record()
  if (def.keyType) assertSerializableZod(def.keyType, stepName);
  if (def.valueType) assertSerializableZod(def.valueType, stepName);
}
```

### New dependency

Add `zod-to-json-schema` to `libs/barnum/package.json`:

```json
{
  "dependencies": {
    "zod-to-json-schema": "^3.x"
  },
  "peerDependencies": {
    "zod": "^3.x"
  }
}
```

`zod` is a peer dependency — handler authors already need it.

## Rust-side: config changes

### Before (`config.rs` — current `TypeScriptAction`)

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TypeScriptAction {
    pub path: String,
    #[serde(default = "default_exported_as")]
    pub exported_as: String,
    #[serde(default)]
    pub step_config: serde_json::Value,
}
```

### After

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TypeScriptAction {
    pub path: String,
    #[serde(default = "default_exported_as")]
    pub exported_as: String,
    #[serde(default)]
    pub step_config: serde_json::Value,
    /// JSON Schema for this step's input value. Produced by JS from Zod.
    /// Used to validate transition values targeting this step.
    #[serde(default)]
    pub value_schema: Option<serde_json::Value>,
}
```

This also updates the generated schema files (`barnum-config-schema.json`, `barnum-config-schema.zod.ts`).

## Rust-side: output validation

### Where validation hooks in

Validation happens in `response.rs` → `validate_response()`. Currently this function checks:
1. Response is a JSON array
2. Each task's `kind` is a valid next step

We add a third check: if the target step has a `value_schema`, validate the task's `value` against it.

### Before (`response.rs` — current `validate_response`)

```rust
pub fn validate_response(
    response: &serde_json::Value,
    current_step: &Step,
) -> Result<Vec<Task>, ResponseValidationError> {
    let serde_json::Value::Array(items) = response else {
        return Err(ResponseValidationError::NotAnArray);
    };

    let mut tasks = Vec::with_capacity(items.len());

    for (i, item) in items.iter().enumerate() {
        let task: Task = serde_json::from_value(item.clone()).map_err(|e| {
            ResponseValidationError::InvalidTaskFormat {
                index: i,
                error: e.to_string(),
            }
        })?;

        // Check valid transition
        if !current_step.next.contains(&task.step) {
            return Err(ResponseValidationError::InvalidTransition {
                from: current_step.name.clone(),
                to: task.step,
                valid: current_step.next.clone(),
            });
        }

        tasks.push(task);
    }

    Ok(tasks)
}
```

### After

```rust
pub fn validate_response(
    response: &serde_json::Value,
    current_step: &Step,
    step_map: &HashMap<&StepName, &Step>,
) -> Result<Vec<Task>, ResponseValidationError> {
    let serde_json::Value::Array(items) = response else {
        return Err(ResponseValidationError::NotAnArray);
    };

    let mut tasks = Vec::with_capacity(items.len());

    for (i, item) in items.iter().enumerate() {
        let task: Task = serde_json::from_value(item.clone()).map_err(|e| {
            ResponseValidationError::InvalidTaskFormat {
                index: i,
                error: e.to_string(),
            }
        })?;

        // Check valid transition
        if !current_step.next.contains(&task.step) {
            return Err(ResponseValidationError::InvalidTransition {
                from: current_step.name.clone(),
                to: task.step,
                valid: current_step.next.clone(),
            });
        }

        // Validate value against target step's JSON Schema
        if let Some(target_step) = step_map.get(&task.step) {
            if let ActionKind::TypeScript(ts) = &target_step.action {
                if let Some(schema) = &ts.value_schema {
                    validate_value_schema(&task.value.0, schema).map_err(|msg| {
                        ResponseValidationError::ValueSchemaViolation {
                            index: i,
                            target_step: task.step.clone(),
                            error: msg,
                        }
                    })?;
                }
            }
        }

        tasks.push(task);
    }

    Ok(tasks)
}

fn validate_value_schema(
    value: &serde_json::Value,
    schema: &serde_json::Value,
) -> Result<(), String> {
    let compiled = jsonschema::validator_for(schema)
        .map_err(|e| format!("invalid JSON Schema: {e}"))?;
    let errors: Vec<String> = compiled
        .iter_errors(value)
        .map(|e| e.to_string())
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}
```

### New error variant

```rust
pub enum ResponseValidationError {
    NotAnArray,
    InvalidTaskFormat { index: usize, error: String },
    InvalidTransition { from: StepName, to: StepName, valid: Vec<StepName> },
    // NEW:
    ValueSchemaViolation {
        index: usize,
        target_step: StepName,
        error: String,
    },
}
```

With Display:

```rust
Self::ValueSchemaViolation { index, target_step, error } => {
    write!(f, "task at index {index} targeting '{target_step}' failed schema validation: {error}")
}
```

### Call site changes

`validate_response` gains a `step_map` parameter. Update the single call site in `process_stdout`:

**Before:**

```rust
// response.rs — process_stdout
Ok(output_value) => match validate_response(&output_value, step) {
```

**After:**

```rust
// response.rs — process_stdout (gains step_map parameter)
Ok(output_value) => match validate_response(&output_value, step, step_map) {
```

This means `process_stdout` and `process_submit_result` both gain a `step_map` parameter. The call chain is:

```
Engine::convert_task_result
  → process_submit_result(result, task, step, options, &self.step_map)
    → process_stdout(stdout, task, value, step, options, step_map)
      → validate_response(output, step, step_map)
```

### New Rust dependency

Add `jsonschema` to `crates/barnum_config/Cargo.toml`:

```toml
[dependencies]
jsonschema = "0.28"
```

### Serialized config example

The JSON config sent to Rust after `resolveConfig()` processes the handlers:

```json
{
  "steps": [{
    "name": "Analyze",
    "action": {
      "kind": "TypeScript",
      "path": "/abs/path/to/handler.ts",
      "exportedAs": "default",
      "stepConfig": { "instructions": "analyze the file" },
      "valueSchema": {
        "type": "object",
        "properties": { "file": { "type": "string" } },
        "required": ["file"]
      }
    },
    "next": ["Implement"]
  }]
}
```

## Zod subset restriction

Not all Zod features can be represented as JSON Schema. The JS side must reject schemas that use:
- `.transform()` — arbitrary code, not representable
- `.preprocess()` — same
- `.refine()` / `.superRefine()` — arbitrary predicates
- `.pipe()` — chained transforms
- `.brand()` — TypeScript-only, no runtime meaning

These all produce `ZodEffects`, `ZodPipeline`, or `ZodBranded` internal types. The `assertSerializableZod` function walks the schema tree and rejects them.

The check runs at `run()` time. If a handler uses unsupported features, `run()` throws before the Rust binary starts.

Supported Zod types (representable as JSON Schema):
- `z.string()`, `z.number()`, `z.boolean()`, `z.null()`
- `z.object()`, `z.array()`, `z.tuple()`
- `z.union()`, `z.discriminatedUnion()`, `z.intersection()`
- `z.literal()`, `z.enum()`, `z.nativeEnum()`
- `z.optional()`, `z.nullable()`, `z.default()`
- `z.record()`, `z.map()`
- `z.unknown()`, `z.any()`
- String/number refinements (`.min()`, `.max()`, `.regex()`, `.int()`, etc.) — these map to JSON Schema keywords

## Error scenarios

### Step config validation fails at `run()` time

```
Error: Step "Analyze": stepConfig validation failed:
  Expected string at "instructions", received number
```

`run()` throws synchronously (well, rejects the promise). Rust binary never starts.

### Non-serializable Zod feature at `run()` time

```
Error: Step "Analyze": Zod schema uses unsupported type "ZodEffects".
  Only JSON-Schema-representable types are allowed.
  Remove .transform(), .refine(), .preprocess(), .pipe(), or .brand().
```

Same — `run()` rejects, Rust binary never starts.

### Value schema violation at Rust runtime

Handler returns `[{"kind": "Implement", "value": {"file": 42}}]` but the schema expects `file` to be a string.

Rust logs:

```
WARN task at index 0 targeting 'Implement' failed schema validation: 42 is not of type "string"
```

Treated as invalid response → retries per config (if `retryOnInvalidResponse` is enabled).

## What this does NOT do

- Does not change run-handler.ts (stays a dumb pipe)
- Does not validate Bash action outputs (no schema to validate against)
- Does not implement `.validate()` on `BarnumConfig` (that's a separate convenience method)
