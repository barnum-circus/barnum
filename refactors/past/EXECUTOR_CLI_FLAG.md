# Executor CLI Flag

**Parent:** JS_ACTION_RESOLUTION.md
**Depends on:** Nothing
**Status:** DONE

## What Landed

### 1. cli.cjs: runtime detection and --executor injection

**File:** `libs/barnum/cli.cjs`

`cli.cjs` detects the JS runtime, resolves the executor command, and injects `--executor` before forwarding the user's args to the Rust binary. Errors if the user passes `--executor` directly.

```javascript
// --executor is internal. Error if the user passed it directly.
var userArgs = process.argv.slice(2);
if (userArgs.includes('--executor')) {
  console.error('Error: --executor is an internal flag and cannot be passed directly.');
  process.exit(1);
}

var executorPath = path.resolve(__dirname, 'actions', 'executor.ts');

function resolveExecutorCommand() {
  if (typeof Bun !== 'undefined') {
    return process.execPath + ' ' + executorPath;
  }
  var tsxPath = require.resolve('tsx/cli');
  return 'node ' + tsxPath + ' ' + executorPath;
}

var executor = resolveExecutorCommand();
var args = userArgs.concat('--executor', executor);
```

### 2. Hidden --executor flag in Rust CLI

**File:** `crates/barnum_cli/src/lib.rs`

```rust
/// Internal: executor command injected by cli.cjs.
/// Not user-facing — hidden from --help.
#[arg(long, hide = true)]
executor: Option<String>,
```

The flag is accepted and ignored. Rust does nothing with it yet. The flag is `Option<String>` (not required) because tests and direct binary invocations don't go through cli.cjs.

## What Comes Next

The remaining work from this doc (threading executor through RunnerConfig/Engine, using it in dispatch_task, BarnumConfig.run() calling cli.cjs) is now tracked in the parent doc **JS_ACTION_RESOLUTION.md** as integration steps that happen after JS_ACTION_HANDLERS lands.
