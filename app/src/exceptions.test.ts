// Tests for exception (override) labels, row states and the store's change flow. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { needsAttention, type OverrideApi, type OverrideInfo, overrideError, overrideLabel, OverrideStore, rowState } from "./exceptions.ts";
import type { CompileStatus } from "./status.ts";

const info = (over: Partial<OverrideInfo>): OverrideInfo => ({
  id: "a1",
  kind: "page_break",
  params: { to_odd: false },
  anchor_preview: "== The Arithmetic of Light",
  pos_utf16: 120,
  detached: false,
  applicable: true,
  error: null,
  ...over,
});

test("labels: kind and parameters", () => {
  assert.equal(overrideLabel(info({})), "Page break");
  assert.equal(overrideLabel(info({ params: { to_odd: true } })), "Page break · right-hand page");
  assert.equal(overrideLabel(info({ kind: "column_break", params: {} })), "Column break");
  assert.equal(overrideLabel(info({ kind: "space_before", params: { length: "18pt" } })), "Space before 18pt");
  assert.equal(overrideLabel(info({ kind: "figure_placement", params: { placement: "top" } })), "Figure: top");
  assert.equal(overrideLabel(info({ kind: "image_quality", params: { quality: "screen" } })), "Image quality: Screen");
  assert.equal(overrideLabel(info({ kind: "image_quality", params: { quality: "huge" } })), "Image quality: huge");
  assert.equal(overrideLabel(info({ kind: "table_style", params: { style: "grid" } })), "Table: Grid");
  assert.equal(overrideLabel(info({ kind: null, params: {} })), "Invalid exception");
});

test("errors: the kind/parameter prefix goes, the rest stays readable", () => {
  assert.equal(overrideError('space_before.length: "12px" is not a length (like 12pt, 2.5cm, 1in or 0.6em).'), '"12px" is not a length (like 12pt, 2.5cm, 1in or 0.6em).');
  assert.equal(overrideError(new Error("space_before.length: 300pt is out of range (0pt to 288pt or 0em to 20em).")), "300pt is out of range (0pt to 288pt or 0em to 20em).");
  assert.equal(overrideError("figure_placement: placement must be top, bottom, here or auto."), "Placement must be top, bottom, here or auto.");
  assert.equal(overrideError("There is no figure here."), "There is no figure here.");
  assert.equal(overrideError("table_style: style must be plain, academic, grid or striped."), "Style must be plain, academic, grid or striped.");
  assert.equal(overrideError("image_quality: quality must be original, print, screen or email."), "Quality must be original, print, screen or email.");
});

test("row states: detached, not applied (with the diagnostic's reason), invalid", () => {
  const ok = rowState(info({}));
  assert.deepEqual(ok.badges, []);
  assert.equal(ok.reason, null);
  assert.equal(ok.editable, true);
  assert.equal(rowState(info({ kind: "column_break", params: {} })).editable, false);

  const detached = rowState(info({ detached: true }));
  assert.deepEqual(detached.badges.map((b) => b.kind), ["detached"]);
  assert.equal(detached.reason, null);

  const why = "This page break is not applied: its position is inside raw text, a comment, math, code or a function's argument.";
  const gone = "The text this page break was anchored to was deleted; it applies where that text was.";
  const notApplied = rowState(info({ applicable: false }), undefined, [gone, why]);
  assert.deepEqual(notApplied.badges.map((b) => b.text), ["not applied"]);
  assert.equal(notApplied.reason, why);
  assert.equal(notApplied.badges[0].title, why);
  assert.equal(rowState(info({ applicable: false })).reason, "Not applied at this position.");
  // Not compiled yet: no verdict.
  assert.deepEqual(rowState(info({ applicable: null })).badges, []);

  // The compile status wins over the listing.
  assert.deepEqual(rowState(info({ applicable: true }), { id: "a1", applied: false, detached: true }).badges.map((b) => b.kind), ["detached", "not-applied"]);
  assert.deepEqual(rowState(info({ applicable: false, detached: true }), { id: "a1", applied: true, detached: false }).badges, []);

  const invalid = rowState(info({ kind: null, params: {}, pos_utf16: null, applicable: null, error: "figure_placement: placement must be top, bottom, here or auto." }));
  assert.deepEqual(invalid.badges.map((b) => b.kind), ["invalid"]);
  assert.equal(invalid.editable, false);
  assert.equal(invalid.reason, "Placement must be top, bottom, here or auto.");

  // Needs attention: any badge.
  assert.equal(needsAttention(ok), false);
  assert.ok(needsAttention(detached) && needsAttention(notApplied) && needsAttention(invalid));
});

/** A fake backend that logs calls in order. */
function fakeApi(fail = false) {
  const log: string[] = [];
  let list: OverrideInfo[] = [info({})];
  const api: OverrideApi = {
    list: async () => {
      log.push("list");
      return list;
    },
    add: async (kind, _params, pos) => {
      log.push(`add ${kind} ${pos}`);
      if (fail) throw 'space_before.length: "12px" is not a length (like 12pt, 2.5cm, 1in or 0.6em).';
      list = [...list, info({ id: "b2", kind, pos_utf16: pos })];
      return "b2";
    },
    remove: async (id) => {
      log.push(`remove ${id}`);
      list = list.filter((o) => o.id !== id);
    },
    update: async (id) => {
      log.push(`update ${id}`);
    },
    batch: async (ops) => {
      log.push(`batch ${ops.map((o) => `${o.op} ${o.id}`).join(", ")}`);
      const removed = new Set(ops.filter((o) => o.op === "remove").map((o) => o.id));
      list = list.filter((o) => !removed.has(o.id));
    },
    drain: async () => {
      await new Promise((r) => setTimeout(r, 5));
      log.push("drain");
    },
  };
  return { api, log };
}

test("store: every change drains first and reloads after, one at a time", async () => {
  const { api, log } = fakeApi();
  const store = new OverrideStore(api);
  let changes = 0;
  store.subscribe(() => changes++);
  const added = store.add("space_before", { length: "12pt" }, 40);
  const removed = store.remove("a1");
  assert.equal(await added, "b2");
  await removed;
  assert.deepEqual(log, ["drain", "add space_before 40", "list", "drain", "remove a1", "list"]);
  assert.deepEqual(store.list.map((o) => o.id), ["b2"]);
  assert.ok(changes >= 2);
  assert.equal(store.label("b2"), "Space before");
});

test("store: a batch is one change (drain, batch, reload)", async () => {
  const { api, log } = fakeApi();
  const store = new OverrideStore(api);
  await store.add("space_before", { length: "12pt" }, 40);
  await store.batch([
    { op: "update", id: "b2", params: { length: "1em" } },
    { op: "remove", id: "a1" },
  ]);
  assert.deepEqual(log.slice(3), ["drain", "batch update b2, remove a1", "list"]);
  assert.deepEqual(store.list.map((o) => o.id), ["b2"]);
});

test("store: a failed change still reloads and rejects with a readable message", async () => {
  const { api, log } = fakeApi(true);
  const store = new OverrideStore(api);
  await assert.rejects(store.add("space_before", { length: "12px" }, 3), { message: '"12px" is not a length (like 12pt, 2.5cm, 1in or 0.6em).' });
  assert.deepEqual(log, ["drain", "add space_before 3", "list"]);
  // The chain goes on after a failure.
  await store.update("a1", { to_odd: true });
  assert.deepEqual(log.slice(3), ["drain", "update a1", "list"]);
});

test("store: compile statuses and override diagnostics feed the rows, then a reload", async () => {
  const { api, log } = fakeApi();
  const store = new OverrideStore(api);
  await store.refresh();
  const status: CompileStatus = {
    state: "done",
    version: 3,
    pages_version: 3,
    pages: 4,
    page_sizes: [],
    diagnostics: [
      { severity: "warning", message: "This page break is not applied: here.", hints: [], location: { in: "override", id: "a1" } },
      { severity: "warning", message: "elsewhere", hints: [], location: { in: "unknown" } },
    ],
    overrides: [{ id: "a1", applied: false, detached: false }],
    duration_ms: 10,
  };
  store.onStatus({ ...status, state: "compiling", overrides: [] });
  assert.deepEqual(log, ["list"]);
  store.onStatus(status);
  const state = store.state(store.list[0]);
  assert.deepEqual(state.badges.map((b) => b.kind), ["not-applied"]);
  assert.equal(state.reason, "This page break is not applied: here.");
  await store.refresh();
  assert.deepEqual(log, ["list", "list", "list"]);
});

test("store: answers for a previous project are dropped", async () => {
  const { api } = fakeApi();
  const store = new OverrideStore(api);
  const loading = store.refresh();
  store.reset();
  await loading;
  assert.deepEqual(store.list, []);
});
