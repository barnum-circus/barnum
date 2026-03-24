#!/usr/bin/env node
'use strict';

var path = require('path');
var spawn = require('child_process').spawn;
var chmodSync = require('fs').chmodSync;

var bin;

if (process.platform === 'darwin' && process.arch === 'x64') {
  bin = path.join(__dirname, 'artifacts', 'macos-x64', 'barnum');
} else if (process.platform === 'darwin' && process.arch === 'arm64') {
  bin = path.join(__dirname, 'artifacts', 'macos-arm64', 'barnum');
} else if (process.platform === 'linux' && process.arch === 'x64') {
  bin = path.join(__dirname, 'artifacts', 'linux-x64', 'barnum');
} else if (process.platform === 'linux' && process.arch === 'arm64') {
  bin = path.join(__dirname, 'artifacts', 'linux-arm64', 'barnum');
} else if (process.platform === 'win32' && process.arch === 'x64') {
  bin = path.join(__dirname, 'artifacts', 'win-x64', 'barnum.exe');
} else {
  throw new Error(
    `Platform "${process.platform} (${process.arch})" not supported.`
  );
}

// --executor and --run-handler-path are internal. Error if the user passed them directly.
var userArgs = process.argv.slice(2);
var internalFlags = ['--executor', '--run-handler-path'];
for (var flag of internalFlags) {
  if (userArgs.includes(flag)) {
    console.error('Error: ' + flag + ' is an internal flag and cannot be passed directly.');
    process.exit(1);
  }
}

var executorPath = path.resolve(__dirname, 'actions', 'executor.ts');
var runHandlerPath = path.resolve(__dirname, 'actions', 'run-handler.ts');

function resolveExecutorCommand() {
  if (typeof Bun !== 'undefined') {
    // Bun runs .ts natively
    return process.execPath + ' ' + executorPath;
  }
  // Node: use tsx
  var tsxPath = require.resolve('tsx/cli');
  return 'node ' + tsxPath + ' ' + executorPath;
}

var executor = resolveExecutorCommand();
var args = userArgs.concat('--executor', executor, '--run-handler-path', runHandlerPath);

try {
  chmodSync(bin, 0o755);
} catch {}

spawn(bin, args, { stdio: 'inherit' }).on('exit', process.exit);
