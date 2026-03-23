'use strict';

const path = require('path');

let binary;

if (process.platform === 'darwin' && process.arch === 'x64') {
  binary = path.join(__dirname, 'artifacts', 'macos-x64', 'barnum');
} else if (process.platform === 'darwin' && process.arch === 'arm64') {
  binary = path.join(__dirname, 'artifacts', 'macos-arm64', 'barnum');
} else if (process.platform === 'linux' && process.arch === 'x64') {
  binary = path.join(__dirname, 'artifacts', 'linux-x64', 'barnum');
} else if (process.platform === 'linux' && process.arch === 'arm64') {
  binary = path.join(__dirname, 'artifacts', 'linux-arm64', 'barnum');
} else if (process.platform === 'win32' && process.arch === 'x64') {
  binary = path.join(__dirname, 'artifacts', 'win-x64', 'barnum.exe');
} else {
  throw new Error(
    `Platform "${process.platform} (${process.arch})" not supported.`
  );
}

module.exports = binary;
