// Tests for the About dialog's facts and diagnostic text. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { aboutFacts, type AppInfo, diagnosticText } from "./about.ts";

const info: AppInfo = { version: "0.1.0", git: "1a2b3c4-dirty", protocol: 2, level: 1, os: "windows", arch: "x86_64" };

test("the facts name the build, the protocol, the level and the server", () => {
  assert.deepEqual(aboutFacts(info), [
    ["Version", "0.1.0 (1a2b3c4-dirty)"],
    ["Protocol", "2"],
    ["Feature level", "1"],
    ["System", "windows x86_64"],
    ["Server", "Not connected"],
  ]);
  assert.equal(aboutFacts({ ...info, server: "0.2.0" })[4][1], "0.2.0");
});

test("the diagnostic info is plain text, one fact per line", () => {
  assert.equal(diagnosticText({ ...info, server: "0.2.0" }), "Lethora\nVersion: 0.1.0 (1a2b3c4-dirty)\nProtocol: 2\nFeature level: 1\nSystem: windows x86_64\nServer: 0.2.0\n");
});
