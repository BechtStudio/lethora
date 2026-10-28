// Tests for live presence: remote carets, the online row and the sender's throttle. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { EditorState } from "@codemirror/state";
import { caretsOf, inkOn, initials, onlineRow, onlineUsers, type PeerPresence, PresenceSender, remoteCaretField, safeColour, setRemoteCarets, type Here } from "./presence.ts";

const peer = (over: Partial<PeerPresence> = {}): PeerPresence => ({
  device: "d1",
  user: "u1",
  name: "Bea Lind",
  colour: "#009e73",
  role: "editor",
  view: "author",
  anchor: 2,
  head: 5,
  ...over,
});

test("initials, colours and readable ink", () => {
  assert.equal(initials("Bea Lind"), "BL");
  assert.equal(initials("  ada  "), "A");
  assert.equal(initials("Jean Luc de Vries"), "JV");
  assert.equal(initials("Ølaf"), "Ø");
  assert.equal(initials(""), "?");
  assert.equal(safeColour("#0072B2"), "#0072B2");
  assert.equal(safeColour("red; background: url(x)"), "#8e6c8a");
  assert.equal(inkOn("#0072b2"), "#ffffff");
  assert.equal(inkOn("#e69f00"), "#1d1c1a");
  assert.equal(inkOn("#56b4e9"), "#1d1c1a");
});

test("carets: author and science views only, placed ones, mapped and clamped", () => {
  const peers = [
    peer(),
    peer({ device: "d2", view: "science", anchor: 9, head: 40 }),
    peer({ device: "d3", view: "publisher" }),
    peer({ device: "d4", anchor: undefined, head: undefined }),
    peer({ device: "d5", name: undefined, colour: "nope" }),
  ];
  const carets = caretsOf(peers, (pos) => pos + 1, 20);
  assert.deepEqual(carets, [
    { device: "d1", name: "Bea Lind", colour: "#009e73", anchor: 3, head: 6 },
    { device: "d2", name: "Bea Lind", colour: "#009e73", anchor: 10, head: 20 },
    { device: "d5", name: "Someone", colour: "#8e6c8a", anchor: 3, head: 6 },
  ]);
});

test("the online row: each user once, by name, with role and views", () => {
  const row = onlineUsers([
    peer({ device: "d2", view: "science" }),
    peer({ device: "c", user: "u2", name: "Carl", colour: "#cc79a7", role: "reader", view: "publisher" }),
    peer(),
    peer({ device: "x", user: "u3", name: undefined, role: "owner" }),
  ]);
  assert.deepEqual(
    row.map((u) => [u.user, u.initials, u.title]),
    [
      ["u1", "BL", "Bea Lind · Editor · Author and science views"],
      ["u2", "C", "Carl · Reader · Publisher view"],
      ["u3", "S", "Someone · Owner · Author view"],
    ],
  );
  assert.equal(row[1].ink, inkOn("#cc79a7"));
  assert.deepEqual(onlineUsers([]), []);
});

test("the online row shows five users, the rest behind +N", () => {
  const users = onlineUsers(Array.from({ length: 7 }, (_, i) => peer({ device: `d${i}`, user: `u${i}`, name: `User ${i}` })));
  const { shown, rest } = onlineRow(users);
  assert.deepEqual(shown.map((u) => u.user), ["u0", "u1", "u2", "u3", "u4"]);
  assert.deepEqual(rest.map((u) => u.user), ["u5", "u6"]);
  assert.deepEqual(onlineRow(users.slice(0, 5)).rest, []);
});

test("remote carets follow edits and count their moves", () => {
  let state = EditorState.create({ doc: "Hello world", extensions: remoteCaretField });
  const set = (anchor: number, head: number) =>
    (state = state.update({ effects: setRemoteCarets([{ device: "d1", name: "Bea", colour: "#009e73", anchor, head }]) }).state);
  set(6, 11);
  assert.deepEqual(state.field(remoteCaretField).map((c) => [c.anchor, c.head, c.moves]), [[6, 11, 1]]);
  // Typing before the selection moves it along.
  state = state.update({ changes: { from: 0, insert: "Oh, " } }).state;
  assert.deepEqual(state.field(remoteCaretField).map((c) => [c.anchor, c.head, c.moves]), [[10, 15, 1]]);
  // The same caret again: not a move; another place: one.
  set(10, 15);
  assert.equal(state.field(remoteCaretField)[0].moves, 1);
  set(10, 12);
  assert.equal(state.field(remoteCaretField)[0].moves, 2);
  // Gone with the next list.
  state = state.update({ effects: setRemoteCarets([]) }).state;
  assert.deepEqual(state.field(remoteCaretField), []);
});

test("the sender sends at most every gap, the latest, nothing unchanged", async () => {
  let now = 1000;
  const sent: Here[] = [];
  const timers: Array<() => void> = [];
  const sender = new PresenceSender(
    async (here) => void sent.push(here),
    100,
    () => now,
    (run) => void timers.push(run),
  );
  const here = (head: number): Here => ({ view: "author", anchor: head, head });
  sender.update(here(1));
  assert.deepEqual(sent, [here(1)]);
  now = 1020;
  sender.update(here(2));
  sender.update(here(3));
  assert.equal(sent.length, 1);
  assert.equal(timers.length, 1);
  now = 1100;
  timers.shift()!();
  assert.deepEqual(sent, [here(1), here(3)]);
  now = 1300;
  sender.update(here(3));
  assert.equal(sent.length, 2);
  sender.update({ ...here(3), view: "science" });
  assert.equal(sent.length, 3);
  // After a reset the same presence goes out again (another project).
  sender.reset();
  now = 1500;
  sender.update({ ...here(3), view: "science" });
  assert.equal(sent.length, 4);
});
