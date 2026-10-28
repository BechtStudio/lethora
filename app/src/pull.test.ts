// Tests for the pull schedule and the status cloud's text. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { cloudKind, cloudTitle, colourName, deviceName, inviteSummary, isInviteLink, isPlainHttp, joinedName, memberName, notFoundMessage, Poller, readOnlyRole, reviewReadOnly, serverHost, type MemberInfo, type SyncStatus, type Timers } from "./pull.ts";

const settle = () => new Promise<void>((resolve) => setImmediate(resolve));

/** Timers fired by hand. */
function manualTimers() {
  const callbacks = new Map<number, () => void>();
  let next = 0;
  const timers: Timers = {
    every: (f) => {
      callbacks.set(++next, f);
      return next;
    },
    cancel: (id) => callbacks.delete(id as number),
  };
  return { timers, fire: () => [...callbacks.values()].forEach((f) => f()), count: () => callbacks.size };
}

test("pulls never overlap: ticks are skipped while one is queued or in flight", async () => {
  const { timers, fire, count } = manualTimers();
  const releases: Array<() => void> = [];
  let runs = 0;
  let inFlight = 0;
  let maxInFlight = 0;
  const poller = new Poller(
    () => {
      runs++;
      inFlight++;
      maxInFlight = Math.max(maxInFlight, inFlight);
      return new Promise<void>((resolve) => releases.push(() => (inFlight--, resolve())));
    },
    { timers },
  );
  poller.start();
  poller.start();
  assert.equal(count(), 1, "one timer");
  fire();
  fire();
  fire();
  assert.equal(runs, 1);
  assert.ok(poller.running);
  releases.shift()!();
  await settle();
  assert.ok(!poller.running);
  fire();
  assert.equal(runs, 2);
  assert.equal(maxInFlight, 1);

  poller.stop();
  assert.equal(count(), 0);
  releases.shift()!();
  await settle();
  assert.equal(poller.tick(), false, "stopped for good");
  poller.start();
  assert.equal(count(), 0);
});

test("a failed pull is reported and the next tick runs again", async () => {
  const errors: unknown[] = [];
  let runs = 0;
  const poller = new Poller(
    async () => {
      runs++;
      throw "No project is open.";
    },
    { timers: manualTimers().timers, onError: (e) => errors.push(e) },
  );
  assert.equal(poller.tick(), true);
  await settle();
  assert.equal(poller.tick(), true);
  await settle();
  assert.equal(runs, 2);
  assert.deepEqual(errors, ["No project is open.", "No project is open."]);
});

test("the status cloud: look and tooltip per state", () => {
  const status = (s: Partial<SyncStatus>): SyncStatus => ({ mode: "encrypted", state: "ok", ...s });
  const time = () => "14:05";
  assert.equal(cloudKind(status({ mode: "off" })), "off");
  assert.match(cloudTitle(status({ mode: "off" }), time), /Sync is off/);
  assert.equal(cloudKind(status({ lastSync: 1 })), "ok");
  assert.equal(cloudTitle(status({ lastSync: 1 }), time), "Synced (encrypted) — last sync 14:05");
  assert.equal(cloudKind(status({ mode: "plain", provider: "Dropbox" })), "plain");
  assert.equal(cloudTitle(status({ mode: "plain", provider: "Dropbox" })), "Synced in plain text by Dropbox");
  assert.match(cloudTitle(status({ mode: "plain" })), /plain text/);
  for (const state of ["unavailable", "needsCode", "error"] as const) {
    assert.equal(cloudKind(status({ state })), "warning");
    assert.ok(cloudTitle(status({ state })).length > 0);
    assert.equal(cloudTitle(status({ state, message: "USB stick unplugged" })), "USB stick unplugged");
  }
});

test("a joined project is named after the sync folder", () => {
  assert.equal(joinedName("C:\\Users\\a\\Dropbox\\Novel.lethora-sync"), "Novel");
  assert.equal(joinedName("/home/a/Dropbox/Novel 2.lethora-sync/"), "Novel 2");
  assert.equal(joinedName("/media/stick/Essay"), "Essay");
  assert.equal(joinedName("/"), "Synced project");
});

test("a code that finds nothing names where Lethora looked", () => {
  assert.equal(notFoundMessage(["Dropbox"], null), "No synced project with this code was found in Dropbox.");
  assert.equal(notFoundMessage(["Dropbox", "OneDrive"], null), "No synced project with this code was found in Dropbox and OneDrive.");
  assert.equal(notFoundMessage(["Dropbox", "OneDrive – Uni", "iCloud Drive"], null), "No synced project with this code was found in Dropbox, OneDrive – Uni and iCloud Drive.");
  assert.match(notFoundMessage([], null), /No sync app/);
  assert.equal(notFoundMessage(["Dropbox"], "E:\Stick"), "No synced project with this code is in E:\Stick.");
});

test("a requested pull runs now, or once more right after the one in flight", async () => {
  const releases: Array<() => void> = [];
  let runs = 0;
  const poller = new Poller(() => {
    runs++;
    return new Promise<void>((resolve) => releases.push(resolve));
  }, { timers: manualTimers().timers });
  poller.request();
  assert.equal(runs, 1);
  poller.request();
  poller.request();
  assert.equal(runs, 1, "not two at once");
  releases.shift()!();
  await settle();
  assert.equal(runs, 2, "once more, not twice");
  releases.shift()!();
  await settle();
  assert.equal(runs, 2);
  poller.stop();
  poller.request();
  assert.equal(runs, 2);
});

test("the status cloud for server sync", () => {
  const status = (s: Partial<SyncStatus>): SyncStatus => ({ mode: "server", state: "online", server: "https://lethora.example:8443", ...s });
  const time = () => "14:05";
  const secs = () => "14:05:30";
  assert.equal(cloudKind(status({})), "ok");
  assert.equal(cloudTitle(status({ lastSync: 1 }), time, secs), "Synced with lethora.example:8443 (encrypted) — last sync 14:05");
  assert.equal(cloudKind(status({ state: "connecting" })), "connecting");
  assert.equal(cloudTitle(status({ state: "connecting" }), time, secs), "Connecting to lethora.example:8443…");
  assert.equal(cloudKind(status({ state: "offline" })), "warning");
  assert.equal(
    cloudTitle(status({ state: "offline", message: "Can't reach the server: refused.", retryAt: 1 }), time, secs),
    "Offline: Can't reach the server: refused. Trying again at 14:05:30. Your work is saved on this device.",
  );
  for (const state of ["removed", "needsCode", "error"] as const) {
    assert.equal(cloudKind(status({ state })), "warning");
    assert.ok(cloudTitle(status({ state })).length > 0);
    assert.equal(cloudTitle(status({ state, message: "Removed." })), "Removed.");
  }
});

test("invite links, codes and plain http addresses", () => {
  assert.ok(isInviteLink(" https://lethora.example/join/00112233445566778899aabbccddeeff/0123456789abcdef0123456789abcdef#abc "));
  assert.ok(isInviteLink("http://192.168.1.5:8080/join/x/y#z"));
  assert.ok(!isInviteLink("ABCD-EFGH-JKMN"));
  assert.ok(!isInviteLink("https://lethora.example/"));
  assert.ok(isPlainHttp("http://192.168.1.5:8080"));
  assert.ok(isPlainHttp(" WS://nas.local"));
  assert.ok(!isPlainHttp("https://lethora.example"));
  assert.equal(serverHost("https://lethora.example:8443"), "lethora.example:8443");
  assert.equal(serverHost("http://[::1]:80/x"), "[::1]:80");
});

test("reviewers and readers on a server are read only; everyone else writes", () => {
  const server = (role?: SyncStatus["role"]): SyncStatus => ({ mode: "server", state: "online", role });
  assert.equal(readOnlyRole(server("reader")), "reader");
  assert.equal(readOnlyRole(server("reviewer")), "reviewer");
  assert.equal(readOnlyRole(server("editor")), null);
  assert.equal(readOnlyRole(server("owner")), null);
  assert.equal(readOnlyRole(server()), null);
  assert.equal(readOnlyRole({ mode: "off", state: "ok" }), null);
  assert.equal(readOnlyRole(null), null);
  // The review is read only for readers alone.
  assert.equal(reviewReadOnly(server("reader")), true);
  assert.equal(reviewReadOnly(server("reviewer")), false);
  assert.equal(reviewReadOnly(server("owner")), false);
  assert.equal(reviewReadOnly({ mode: "off", state: "ok" }), false);
  assert.equal(reviewReadOnly(null), false);
});

test("members, devices and invites read as names", () => {
  const alice: MemberInfo = { user: "a1b2c3d4e5", name: "Alice", colour: "#0072b2", role: "owner", removed: false, devices: ["d1"] };
  assert.equal(memberName(alice), "Alice");
  assert.equal(memberName({ user: "a1b2c3d4e5", name: "  " }), "Member a1b2c3");
  assert.equal(deviceName("d1", "d1"), "This device");
  assert.equal(deviceName("ffee00aa11", "d1"), "Device ffee00");
  const date = (ms: number) => new Date(ms).toISOString().slice(0, 10);
  const invite = { id: "i", creator: "d1", creator_user: alice.user, role: "editor" as const, expires: 86400, uses_left: 1 };
  assert.equal(inviteSummary(invite, [alice], date), "Editor · by Alice · until 1970-01-02 · 1 use left");
  assert.equal(inviteSummary({ ...invite, role: undefined, uses_left: 5 }, [], date), "Device · by a former member · until 1970-01-02 · 5 uses left");
});

test("profile colours have names for screen readers", () => {
  const palette = ["#0072b2", "#d55e00", "#123456"];
  assert.equal(colourName("#0072b2", palette), "Blue");
  assert.equal(colourName("#D55E00", palette), "Vermilion");
  assert.equal(colourName("#123456", palette), "Colour 3");
});
