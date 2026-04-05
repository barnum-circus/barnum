# Branded IDs on the TypeScript Side

## Motivation

All IDs in the TypeScript AST are bare `number`. `effect_id` on `HandleAction` and `PerformAction` is `number`. Nothing prevents passing a handler ID where an effect ID is expected, or mixing IDs from different namespaces.

When the Handle/Perform split happens (see RESUME_VS_RESTART_HANDLERS.md), there will be three ID types: `ResumeHandlerId`, `RestartHandlerId`, `BreakHandlerId`. These must not be interchangeable. Branded types enforce this at compile time with zero runtime cost.

## What changes

### 1. Define branded ID types

**Before** (`effect-id.ts`):

```ts
let nextEffectId = 0;

export function allocateEffectId(): number {
  return nextEffectId++;
}
```

**After:**

```ts
type EffectId = number & { readonly __brand: unique symbol };

let nextEffectId = 0;

export function allocateEffectId(): EffectId {
  return nextEffectId++ as EffectId;
}
```

The `unique symbol` brand prevents accidental assignment between branded types. Only `allocateEffectId` can produce an `EffectId` (via the cast). Consuming code sees `EffectId`, not `number`.

### 2. Update AST interfaces

**Before** (`ast.ts:46`):

```ts
export interface HandleAction {
  kind: "Handle";
  effect_id: number;
  body: Action;
  handler: Action;
}

export interface PerformAction {
  kind: "Perform";
  effect_id: number;
}
```

**After:**

```ts
export interface HandleAction {
  kind: "Handle";
  effect_id: EffectId;
  body: Action;
  handler: Action;
}

export interface PerformAction {
  kind: "Perform";
  effect_id: EffectId;
}
```

### 3. Update all allocation sites

Every call to `allocateEffectId()` already returns `EffectId` after step 1. The local variables storing the result need type annotation or will be inferred:

| File | Line | Current | After |
|------|------|---------|-------|
| `bind.ts:127` | `effectIds[i]` | `number` | `EffectId` (inferred from allocateEffectId) |
| `try-catch.ts:33` | `const effectId = allocateEffectId()` | `number` | `EffectId` |
| `race.ts:58` | `const effectId = allocateEffectId()` | `number` | `EffectId` |
| `race.ts:142` | `const effectId = allocateEffectId()` | `number` | `EffectId` |
| `ast.ts:803` | `const effectId = allocateEffectId()` (recur) | `number` | `EffectId` |
| `ast.ts:839` | `const effectId = allocateEffectId()` (earlyReturn) | `number` | `EffectId` |
| `ast.ts:897` | `const effectId = allocateEffectId()` (loop) | `number` | `EffectId` |

These should all just work via inference — `allocateEffectId()` returns `EffectId`, so the locals are `EffectId`. The AST interfaces accept `EffectId`. No manual casts needed at call sites.

### 4. buildLoopAction parameter

**Before** (`ast.ts:863`):

```ts
function buildLoopAction(effectId: number, body: Action): Action {
```

**After:**

```ts
function buildLoopAction(effectId: EffectId, body: Action): Action {
```

## When Handle/Perform splits into two kinds

When RESUME_VS_RESTART_HANDLERS.md is implemented, the single `EffectId` becomes two types:

```ts
type ResumeHandlerId = number & { readonly __brand: unique symbol };
type RestartHandlerId = number & { readonly __brand: unique symbol };
```

Each gets its own allocator. The AST interfaces use the matching type:

```ts
export interface ResumeHandleAction {
  kind: "ResumeHandle";
  resume_handler_id: ResumeHandlerId;
  ...
}

export interface ResumePerformAction {
  kind: "ResumePerform";
  resume_handler_id: ResumeHandlerId;
}
```

A `ResumeHandlerId` cannot be passed where a `RestartHandlerId` is expected. The compiler catches it.

## Sequencing

This can land independently of the three-way Handle split. Step 1: brand the existing `EffectId`. Step 2 (later): split into three branded types when the Handle split lands. Step 1 is a pure type-level change — no runtime behavior changes, no AST serialization changes (branded types erase to `number` at runtime).
