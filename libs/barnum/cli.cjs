#!/usr/bin/env node
"use strict";

const { execFileSync } = require("child_process");
const path = require("path");
const os = require("os");
const fs = require("fs");

const platform = os.platform();
const arch = os.arch();

let binaryName = "barnum";
let artifactDir;

if (platform === "darwin" && arch === "arm64") artifactDir = "macos-arm64";
else if (platform === "darwin") artifactDir = "macos-x64";
else if (platform === "linux" && arch === "arm64") artifactDir = "linux-arm64";
else if (platform === "linux") artifactDir = "linux-x64";
else if (platform === "win32") { artifactDir = "win-x64"; binaryName = "barnum.exe"; }
else { console.error(`Unsupported platform: ${platform}-${arch}`); process.exit(1); }

const binaryPath = path.join(__dirname, "artifacts", artifactDir, binaryName);

if (!fs.existsSync(binaryPath)) {
  console.error(`Binary not found: ${binaryPath}`);
  process.exit(1);
}

try {
  execFileSync(binaryPath, process.argv.slice(2), { stdio: "inherit" });
} catch (e) {
  process.exit(e.status || 1);
}
