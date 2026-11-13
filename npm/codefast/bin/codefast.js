#!/usr/bin/env node
// codefast-npm-wrapper 1
//
// The line above is how `codefast doctor` recognises this file: keep it as the second line.
//
// Runs the codefast binary that npm installed for this platform.
//
// The binary lives in one of the optional dependencies (one package per operating system and
// CPU); npm installs only the one that matches. Nothing is downloaded here and no install
// script runs, so `npx -y PACKAGE mcp` works where lifecycle scripts are disabled.
// Nothing is written to stdout: it belongs to the binary (it carries the MCP protocol).

"use strict";

const { spawn } = require("node:child_process");
const { realpathSync } = require("node:fs");

const scope = require("../package.json").name.split("/")[0];
const exe = process.platform === "win32" ? "codefast.exe" : "codefast";
const platformPackage = `${scope}/codefast-${process.platform}-${process.arch}`;

// True when FILE is this script, by whatever name or link it is reached.
function isThisScript(file) {
  try {
    return realpathSync(file) === realpathSync(__filename);
  } catch {
    return false;
  }
}

let binary = process.env.CODEFAST_BIN;
if (binary && isThisScript(binary)) {
  // CODEFAST_BIN names this wrapper (it is what `which codefast` prints after an npm install).
  // Starting it again would never end, so the variable is ignored and the platform binary runs.
  binary = undefined;
}
if (!binary) {
  try {
    binary = require.resolve(`${platformPackage}/bin/${exe}`);
  } catch {
    console.error(
      `codefast: no prebuilt binary for ${process.platform} ${process.arch} (${platformPackage} is not installed).\n` +
        "codefast: reinstall without --omit=optional, or set CODEFAST_BIN to the path of a codefast binary.",
    );
    process.exit(127);
  }
}

const child = spawn(binary, process.argv.slice(2), { stdio: "inherit", windowsHide: true });

const signals = ["SIGINT", "SIGTERM", "SIGHUP"];
const forward = (signal) => child.kill(signal);
for (const signal of signals) {
  process.on(signal, forward);
}

child.on("error", (error) => {
  console.error(`codefast: cannot run ${binary}: ${error.message}`);
  process.exit(126);
});

child.on("exit", (code, signal) => {
  if (signal) {
    // End the way the binary ended, so that the caller sees the same signal.
    for (const name of signals) {
      process.removeListener(name, forward);
    }
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code ?? 1);
});
