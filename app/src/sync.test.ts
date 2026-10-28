// Tests for the edit queue against a JS model of the Rust core, with
// controllable response timing. Run: npm test (node --test, type stripping).

import { test } from "node:test";
import assert from "node:assert/strict";
import { EditorState, type TransactionSpec } from "@codemirror/state";
import { Sync, fnv1a, textHash, type Backend, type Reply, type TextChange } from "./sync.ts";

const isHigh = (c: number) => c >= 0xd800 && c < 0xdc00;
const isLow = (c: number) => c >= 0xdc00 && c < 0xe000;
const isBoundary = (s: string, pos: number) =>
  pos <= 0 || pos >= s.length || !(isHigh(s.charCodeAt(pos - 1)) && isLow(s.charCodeAt(pos)));

/** Applies a change set (sorted, original offsets), validating like the core. */
function apply(text: string, changes: TextChange[]): string {
  let out = "";
  let pos = 0;
  for (const c of changes) {
    if (c.from < pos || c.to < c.from || c.to > text.length) throw new Error(`invalid change ${c.from}..${c.to}`);
    if (!isBoundary(text, c.from) || !isBoundary(text, c.to)) throw new Error("splits a surrogate pair");
    out += text.slice(pos, c.from) + c.insert;
    pos = c.to;
  }
  return out + text.slice(pos);
}

/** The inverse of `changes` (applied to `text`), in offsets of the result. */
function invert(text: string, changes: TextChange[]): TextChange[] {
  const inverse: TextChange[] = [];
  let delta = 0;
  for (const c of changes) {
    const from = c.from + delta;
    inverse.push({ from, to: from + c.insert.length, insert: text.slice(c.from, c.to) });
    delta += c.insert.length - (c.to - c.from);
  }
  return inverse;
}

/** Model of the core: applies each request when it arrives, answers when released. */
class FakeServer {
  text: string;
  undoStack: TextChange[][] = [];
  redoStack: TextChange[][] = [];
  checkpointTexts: string[] = [];
  responses: Array<() => void> = [];
  log: string[] = [];
  /** The `split` flag of each edit request. */
  splits: boolean[] = [];
  inFlight = 0;
  maxInFlight = 0;
  /** Added to the next length reported by `edit` (to provoke a resync). */
  lengthBias = 0;

  constructor(text: string) {
    this.text = text;
  }

  private respond<T>(name: string, compute: () => T): Promise<T> {
    this.log.push(name);
    this.inFlight++;
    this.maxInFlight = Math.max(this.maxInFlight, this.inFlight);
    let result: { ok: true; value: T } | { ok: false; error: unknown };
    try {
      result = { ok: true, value: compute() };
    } catch (error) {
      result = { ok: false, error };
    }
    return new Promise<T>((resolve, reject) => {
      this.responses.push(() => {
        this.inFlight--;
        if (result.ok) resolve(result.value);
        else reject(result.error);
      });
    });
  }

  private change(changes: TextChange[]): TextChange[] {
    const before = this.text;
    this.text = apply(before, changes);
    return invert(before, changes);
  }

  edit(changes: TextChange[], split: boolean): Promise<number> {
    this.splits.push(split);
    return this.respond("edit", () => {
      const inverse = this.change(changes);
      if (changes.some((c) => c.from !== c.to || c.insert)) {
        this.undoStack.push(inverse);
        this.redoStack = [];
      }
      const bias = this.lengthBias;
      this.lengthBias = 0;
      return this.text.length + bias;
    });
  }

  private step(from: TextChange[][], to: TextChange[][]): Reply | null {
    const changes = from.pop();
    if (!changes) return null;
    to.push(this.change(changes));
    return { changes, len16: this.text.length };
  }

  undo(): Promise<Reply | null> {
    return this.respond("undo", () => this.step(this.undoStack, this.redoStack));
  }

  redo(): Promise<Reply | null> {
    return this.respond("redo", () => this.step(this.redoStack, this.undoStack));
  }

  checkpoint(): number {
    this.checkpointTexts.push(this.text);
    return this.checkpointTexts.length - 1;
  }

  restore(id: number): Promise<Reply> {
    return this.respond("restore", () => {
      const changes = [{ from: 0, to: this.text.length, insert: this.checkpointTexts[id] }];
      this.undoStack.push(this.change(changes));
      this.redoStack = [];
      return { changes, len16: this.text.length };
    });
  }

  /** A text change the core makes on request (like repointing an image); fails beyond the text. */
  replace(from: number, to: number, insert: string): Promise<Reply & { path: string }> {
    return this.respond("replace", () => {
      if (to > this.text.length) throw "stale";
      const changes = [{ from, to, insert }];
      this.undoStack.push(this.change(changes));
      this.redoStack = [];
      return { changes, len16: this.text.length, path: insert };
    });
  }

  /** Other devices' changes, one batch per pull (not undoable here). */
  incoming: TextChange[][] = [];

  /** A pull: imports the next incoming batch, if any. */
  pull(): Promise<Reply & { changed?: boolean }> {
    return this.respond("pull", () => {
      const changes = this.incoming.shift() ?? [];
      this.change(changes);
      return { changes, len16: this.text.length, ...(changes.length ? { changed: true } : {}) };
    });
  }

  hash(): Promise<number> {
    return this.respond("hash", () => fnv1a(this.text));
  }

  /** The Backend view (`text` is taken by the model's state). */
  get backend(): Backend {
    return {
      edit: (c, s) => this.edit(c, s),
      undo: () => this.undo(),
      redo: () => this.redo(),
      restore: (id) => this.restore(id),
      text: () => this.respond("text", () => this.text),
      hash: () => this.hash(),
    };
  }
}

/** Lets every pending promise continuation run. */
const settle = () => new Promise<void>((resolve) => setImmediate(resolve));

function setup(initial = "", checkHash = false) {
  const server = new FakeServer(initial);
  let state = EditorState.create({ doc: initial });
  const frames: Array<() => void> = [];
  const errors: string[] = [];
  let sync: Sync | undefined;
  const editor = {
    get state() {
      return state;
    },
    dispatch(spec: TransactionSpec) {
      const tr = state.update(spec);
      state = tr.state;
      sync?.observe(tr);
    },
  };
  const s = new Sync(server.backend, editor, {
    schedule: (f) => frames.push(f),
    onError: (m) => errors.push(m),
    checkHash,
  });
  sync = s;
  return {
    server,
    sync: s,
    errors,
    text: () => state.doc.toString(),
    type(from: number, to: number, insert: string, userEvent = "input.type") {
      editor.dispatch({ changes: { from, to, insert }, userEvent });
    },
    select(pos: number, userEvent = "select") {
      editor.dispatch({ selection: { anchor: pos }, userEvent });
    },
    async frame() {
      for (const f of frames.splice(0)) f();
      await settle();
    },
    async release() {
      const respond = server.responses.shift();
      assert.ok(respond, "no response to release");
      respond();
      await settle();
    },
    /** Answers everything until all edits and operations are through. */
    async drainAll() {
      let done = false;
      void s.drain().then(() => (done = true));
      for (let i = 0; !done; i++) {
        assert.ok(i < 10000, "drain does not finish");
        await settle();
        if (server.responses.length > 0) server.responses.shift()!();
      }
      frames.length = 0;
    },
  };
}

test("FNV-1a over UTF-16 code units", () => {
  assert.equal(fnv1a(""), 0x811c9dc5);
  assert.equal(fnv1a("a"), 0xe40c292c);
  const doc = EditorState.create({ doc: "line one\nzwei 😀\n\ndrei" }).doc;
  assert.equal(textHash(doc), fnv1a(doc.toString()));
});

test("sends one batch per frame and the server follows", async () => {
  const t = setup("abc");
  t.type(3, 3, "d");
  t.type(0, 1, "X");
  assert.equal(t.server.log.length, 0);
  await t.frame();
  assert.deepEqual(t.server.log, ["edit"]);
  assert.equal(t.server.text, "Xbcd");
  await t.release();
  assert.equal(t.text(), t.server.text);
  assert.deepEqual(t.errors, []);
});

test("typing while an edit is in flight: one request at a time, then the rest", async () => {
  const t = setup("");
  t.type(0, 0, "hello");
  await t.frame();
  t.type(5, 5, " world");
  t.type(0, 0, ">");
  await t.frame();
  assert.equal(t.server.responses.length, 1, "second batch waits for the first response");
  await t.release();
  assert.deepEqual(t.server.log, ["edit", "edit"]);
  await t.release();
  assert.equal(t.server.text, ">hello world");
  assert.equal(t.text(), t.server.text);
  assert.equal(t.server.maxInFlight, 1);
});

test("undo flushes pending edits first, in order", async () => {
  const t = setup("");
  t.type(0, 0, "abc");
  void t.sync.undo();
  await t.drainAll();
  assert.deepEqual(t.server.log, ["edit", "undo"]);
  assert.equal(t.server.text, "");
  assert.equal(t.text(), "");
});

test("typing while an undo is in flight is transformed over the undo", async () => {
  const t = setup("");
  t.type(0, 0, "hello");
  await t.drainAll();
  t.type(5, 5, " there");
  await t.drainAll();
  assert.equal(t.server.text, "hello there");

  void t.sync.undo(); // removes " there"
  await settle();
  assert.deepEqual(t.server.log.at(-1), "undo");
  t.type(0, 0, "A"); // before the undone range
  t.type(9, 9, "B"); // inside it ("Ahello th|ere")
  t.type(t.text().length, t.text().length, "C"); // after it
  await t.frame();
  assert.equal(t.server.responses.length, 1);
  await t.release();
  assert.equal(t.text(), "AhelloBC");
  await t.drainAll();
  assert.equal(t.server.text, "AhelloBC");
  assert.deepEqual(t.errors, []);
});

test("queued undo/redo/undo run in order behind an in-flight edit", async () => {
  const t = setup("x");
  t.type(1, 1, "y");
  await t.frame(); // edit in flight
  t.type(2, 2, "z"); // pending
  void t.sync.undo();
  void t.sync.redo();
  void t.sync.undo();
  await t.drainAll();
  assert.deepEqual(t.server.log, ["edit", "edit", "undo", "redo", "undo"]);
  assert.equal(t.server.text, "xy");
  assert.equal(t.text(), "xy");
  assert.equal(t.server.maxInFlight, 1);
});

test("nothing to undo leaves the text alone", async () => {
  const t = setup("abc");
  void t.sync.undo();
  await t.drainAll();
  assert.equal(t.text(), "abc");
});

test("restore while typing", async () => {
  const t = setup("first draft");
  const id = t.server.checkpoint();
  t.type(0, 5, "second");
  await t.drainAll();
  void t.sync.restore(id);
  await settle();
  t.type(t.text().length, t.text().length, "!");
  await t.drainAll();
  assert.equal(t.server.text, t.text());
  assert.equal(t.text(), "first draft!");
});

test("apply: pending edits go first, the reply applies like a restore; its errors reach the caller", async () => {
  const t = setup('#image("a.png")');
  t.type(0, 0, "x");
  const done = t.sync.apply(() => t.server.replace(9, 14, "b.jpg"));
  await settle();
  t.type(t.text().length, t.text().length, "!"); // typed while it is in flight
  await t.drainAll();
  assert.deepEqual(t.server.log, ["edit", "replace", "edit"]);
  assert.equal((await done)?.path, "b.jpg");
  assert.equal(t.text(), 'x#image("b.jpg")!');
  assert.equal(t.server.text, t.text());

  const failing = assert.rejects(t.sync.apply(() => t.server.replace(99, 99, "")), (e) => e === "stale");
  await t.drainAll();
  await failing;
  assert.equal(t.text(), 'x#image("b.jpg")!');
  assert.deepEqual(t.errors, []);
});

test("a pull applies like a restore: pending edits go first, typing meanwhile is transformed over it", async () => {
  const t = setup("Hello world");
  t.type(11, 11, "!"); // pending, not sent yet
  t.server.incoming.push([{ from: 0, to: 0, insert: ">> " }, { from: 5, to: 5, insert: "," }]);
  const pulled = t.sync.apply(() => t.server.pull());
  await settle();
  assert.deepEqual(t.server.log, ["edit"]);
  await t.release(); // the edit's answer; the pull goes out
  assert.deepEqual(t.server.log, ["edit", "pull"]);
  t.type(0, 0, "A"); // typed while the pull is in flight
  t.type(t.text().length, t.text().length, "?");
  await t.drainAll();
  assert.equal((await pulled)?.changed, true);
  assert.deepEqual(t.server.log, ["edit", "pull", "edit"]);
  assert.equal(t.text(), "A>> Hello, world!?");
  assert.equal(t.server.text, t.text());

  // Nothing new: the text stays.
  assert.equal((await Promise.all([t.sync.apply(() => t.server.pull()), t.drainAll()]))[0]?.changed, undefined);
  assert.equal(t.text(), "A>> Hello, world!?");
  assert.deepEqual(t.errors, []);
});

test("call: runs once pending edits are sent and nothing is in flight; offsets map exactly; errors reach the caller", async () => {
  const t = setup("Hello world");
  t.type(0, 0, "Oh, "); // pending, not sent yet
  let mapped = -1;
  const asked = t.sync.call(async () => {
    // The core has every edit typed before the call.
    mapped = t.sync.toCore(t.text().indexOf("world"));
    t.server.log.push(`call ${t.server.text}`);
    return 42;
  });
  await settle();
  assert.deepEqual(t.server.log, ["edit"]);
  t.type(t.text().length, t.text().length, "!"); // typed while the edit is in flight
  await t.drainAll();
  assert.equal(await asked, 42);
  assert.equal(mapped, 10);
  assert.deepEqual(t.server.log, ["edit", "call Oh, Hello world", "edit"]);
  assert.equal(t.server.text, "Oh, Hello world!");

  const failing = assert.rejects(t.sync.call(() => Promise.reject("refused")), (e) => e === "refused");
  await t.drainAll();
  await failing;
  assert.deepEqual(t.errors, []);
});

test("length mismatch after an edit resyncs from the server", async () => {
  const t = setup("abc");
  t.server.lengthBias = 1;
  t.type(3, 3, "d");
  await t.drainAll();
  assert.equal(t.errors.length, 1);
  assert.ok(t.server.log.includes("text"));
  assert.equal(t.text(), t.server.text);
  t.type(0, 0, "!");
  await t.drainAll();
  assert.equal(t.server.text, "!abcd");
  assert.equal(t.text(), t.server.text);
});

test("a rejected edit resyncs to the server text", async () => {
  const t = setup("abc");
  t.server.text = "s"; // diverged behind the editor's back
  t.type(3, 3, "1234567");
  await t.drainAll();
  assert.equal(t.errors.length, 1);
  assert.equal(t.text(), t.server.text);
});

test("hash check catches silent divergence", async () => {
  const t = setup("", true);
  t.type(0, 0, "a");
  await t.frame();
  t.server.text += "!"; // the length still matches what the edit reported
  await t.drainAll();
  assert.deepEqual(t.server.log, ["edit", "hash", "text"]);
  assert.equal(t.errors.length, 1);
  assert.equal(t.text(), "a!");
});

test("suspend holds edits until resume", async () => {
  const t = setup("");
  await t.sync.suspend();
  t.type(0, 0, "held");
  await t.frame();
  assert.deepEqual(t.server.log, []);
  t.sync.resume();
  await t.drainAll();
  assert.equal(t.server.text, "held");
});

test("undo grouping: continuous typing stays one step", async () => {
  const t = setup("");
  t.type(0, 0, "a");
  await t.drainAll();
  t.type(1, 1, "b");
  await t.drainAll();
  assert.deepEqual(t.server.splits, [false, false]);
});

test("undo grouping: a cursor move ends the step", async () => {
  const t = setup("");
  t.type(0, 0, "ab");
  await t.drainAll();
  t.select(0);
  t.type(0, 0, "c");
  await t.drainAll();
  t.select(1, "select.pointer");
  t.type(1, 1, "d");
  await t.drainAll();
  assert.deepEqual(t.server.splits, [false, true, true]);
});

test("undo grouping: a selection change without a user event does not split", async () => {
  const t = setup("ab");
  t.type(2, 2, "c");
  await t.drainAll();
  t.select(0, "");
  t.type(0, 0, "d");
  await t.drainAll();
  assert.deepEqual(t.server.splits, [false, false]);
});

test("undo grouping: a cursor move with edits pending splits after them (±1 frame)", async () => {
  const t = setup("");
  t.type(0, 0, "a");
  t.select(0);
  t.type(0, 0, "b"); // same frame: travels with "a"
  await t.drainAll();
  t.type(2, 2, "c");
  await t.drainAll();
  assert.deepEqual(t.server.splits, [false, true]);
});

// Enter inserts a blank line, Shift+Enter a forced break: each groups like a single newline.
for (const br of ["\n", "\n\n", " \\\n"]) {
  test(`undo grouping: the paragraph after ${JSON.stringify(br)} starts a new step`, async () => {
    const t = setup("");
    t.type(0, 0, "one");
    await t.drainAll();
    t.type(3, 3, br, "input");
    await t.drainAll();
    t.type(3 + br.length, 3 + br.length, "two");
    await t.drainAll();
    t.type(6 + br.length, 6 + br.length, "!");
    await t.drainAll();
    assert.deepEqual(t.server.splits, [false, false, true, false]);
  });
}

for (const event of ["input.paste", "input.drop", "delete.selection", "delete.cut"]) {
  test(`undo grouping: ${event} is a step of its own`, async () => {
    const t = setup("hello world");
    t.type(11, 11, "!");
    await t.drainAll();
    if (event.startsWith("input")) t.type(0, 0, "XY", event);
    else t.type(0, 6, "", event);
    await t.drainAll();
    t.type(0, 0, "z");
    await t.drainAll();
    assert.deepEqual(t.server.splits, [false, true, true]);
    assert.equal(t.text(), t.server.text);
  });
}

/** Deterministic PRNG (mulberry32). */
function rng(seed: number) {
  return () => {
    seed = (seed + 0x6d2b79f5) | 0;
    let x = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    x = (x + Math.imul(x ^ (x >>> 7), 61 | x)) ^ x;
    return ((x ^ (x >>> 14)) >>> 0) / 4294967296;
  };
}

for (const seed of [1, 2, 3, 4, 5, 6, 7, 8]) {
  test(`randomized edits, undo/redo/restore and timing converge (seed ${seed})`, async () => {
    const random = rng(seed);
    const int = (n: number) => Math.floor(random() * n);
    const pieces = ["a", "bc", "é", "😀", "\n", " ", "xyz", "👩‍👩‍👧", "Ω"];
    const t = setup("Start 😀 text\n");
    const pos = () => {
      const s = t.text();
      let p = int(s.length + 1);
      if (!isBoundary(s, p)) p--;
      return p;
    };
    for (let step = 0; step < 1500; step++) {
      const r = random();
      if (r < 0.4) {
        let from = pos();
        let to = random() < 0.6 ? from : pos();
        if (to < from) [from, to] = [to, from];
        const insert = random() < 0.2 ? "" : Array.from({ length: 1 + int(3) }, () => pieces[int(pieces.length)]).join("");
        t.type(from, to, insert, random() < 0.1 ? "input.paste" : "input.type");
      } else if (r < 0.47) {
        void t.sync.undo();
      } else if (r < 0.52) {
        void t.sync.redo();
      } else if (r < 0.54) {
        if (t.server.checkpointTexts.length > 0 && random() < 0.5) {
          void t.sync.restore(int(t.server.checkpointTexts.length));
        } else {
          t.server.checkpoint();
        }
      } else if (r < 0.75) {
        await t.frame();
      } else if (r < 0.95) {
        if (t.server.responses.length > 0) await t.release();
      } else if (r < 0.97) {
        t.sync.requestSplit();
      } else {
        await settle();
      }
    }
    await t.drainAll();
    assert.equal(t.text(), t.server.text);
    assert.deepEqual(t.errors, []);
    assert.equal(t.server.maxInFlight, 1);
    assert.ok(t.server.log.filter((l) => l === "undo").length > 50);
  });
}

test("offsets map between the core's text and the editor's over unsent edits", async () => {
  const t = setup("Hello world");
  assert.equal(t.sync.toEditor(6), 6);
  t.type(0, 0, "Oh, ");
  t.type(15, 15, "!");
  // Not sent yet: the core still has "Hello world".
  assert.equal(t.sync.toEditor(6), 10);
  assert.equal(t.sync.toCore(10), 6);
  assert.equal(t.sync.toCore(99), 11);
  // Before text inserted at the same place.
  assert.equal(t.sync.toEditor(99), 15);
  await t.drainAll();
  assert.equal(t.sync.toEditor(10), 10);
  assert.equal(t.sync.toCore(16), 16);
});
