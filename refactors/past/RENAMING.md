

## 1. Structural Primitives

| Current | Recommended | Why? |
| :--- | :--- | :--- |
| `sequence` | `pipe` | **Effect uses `pipe`** as its primary combinator. If you use `sequence`, people expect it to take an array of independent effects and run them in order. In your system, it's a linear flow. |
| `traverse` | `forEach` | `traverse` is very "Haskell." In a workflow context, `forEach` or `mapParallel` is more intuitive for a Rust-backed VM. |
| `all` | `parallel` | `all` is ambiguous (is it `Promise.all`?). `parallel` explicitly communicates the VM's intent to fan out. |
| `match` | `branch` | `match` is standard, but `branch` feels more "Workflow-y" and distinguishes from TS pattern matching. |

## 5. The "Call" Node

`call` is generic. Since your leaf nodes are specifically referencing exported functions, you might consider:
* **`task`**: Implies a unit of work.
* **`activity`**: (Temporal terminology) Clear, but perhaps too derivative.
* **`invoke`**: High-level, implies the VM is calling out to an external module.

(Invoke sounds good. )

---
