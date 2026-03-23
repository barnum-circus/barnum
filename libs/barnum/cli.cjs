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

var input = process.argv.slice(2);

try {
  chmodSync(bin, 0o755);
} catch {}

spawn(bin, input, { stdio: 'inherit' }).on('exit', process.exit);
