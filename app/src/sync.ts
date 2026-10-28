// The edit queue between the editor and the Rust core.
//
// Invariant: `pending` holds the local edits not yet sent. Its input length is
// the server's current UTF-16 length (counting requests in flight as applied),
// and applying it to the server text gives the editor text.
//
// All requests go through one FIFO with at most one request in flight, so
// edit batches, undo, redo and restore never overtake each other. Changes the
// core makes (undo/redo/restore) are transformed against local edits typed
// while the request was in flight.
//
// No Tauri imports: the backend is injected, so this runs under `node --test`.

import { Annotation, ChangeSet, type EditorState, type Text, type Transaction, type TransactionSpec } from "@codemirror/state";

/** A change in CodeMirror change-set semantics, UTF-16 offsets. */
export interface TextChange {
  from: number;
  to: number;
  insert: string;
}

/** What undo/redo/restore changed (relative to the text before) and the length after. */
export interface Reply {
  changes: TextChange[];
  len16: number;
  /** Undo/redo changed the asset index (only sent when true). */
  assets?: boolean;
}

export interface Backend {
  /** Applies one batch (starting a new undo step first if `split`); resolves to the new length. */
  edit(changes: TextChange[], split: boolean): Promise<number>;
  undo(): Promise<Reply | null>;
  redo(): Promise<Reply | null>;
  restore(id: number): Promise<Reply>;
  /** The full text, for resyncing. */
  text(): Promise<string>;
  /** FNV-1a over the UTF-16 code units of the text (development check). */
  hash(): Promise<number>;
}

/** The part of an EditorView the queue needs. */
export interface Editor {
  readonly state: EditorState;
  dispatch(spec: TransactionSpec): void;
}

export interface SyncOptions {
  /** Schedules a flush; defaults to the next animation frame. */
  schedule?: (flush: () => void) => void;
  /** Compare text hashes with the server (at most once per second). */
  checkHash?: boolean;
  /** Called with a readable message when the editor had to be resynced. */
  onError?: (message: string) => void;
}

/** Marks transactions that apply the core's changes (not to be sent back). */
export const fromCore = Annotation.define<boolean>();

const FNV_OFFSET = 0x811c9dc5;
const FNV_PRIME = 0x01000193;

/** FNV-1a (32 bit) over UTF-16 code units, continuing from `hash`. */
export function fnv1a(s: string, hash = FNV_OFFSET): number {
  for (let i = 0; i < s.length; i++) {
    hash = Math.imul(hash ^ s.charCodeAt(i), FNV_PRIME) >>> 0;
  }
  return hash;
}

/** FNV-1a of a CodeMirror document, without joining it into one string. */
export function textHash(doc: Text): number {
  let hash = FNV_OFFSET;
  for (const iter = doc.iter(); !iter.next().done; ) hash = fnv1a(iter.value, hash);
  return hash;
}

/** User events that form an undo step of their own. */
export const OWN_STEP = ["input.paste", "input.drop", "delete.selection", "delete.cut", "delete.join"];

function insertsLineBreak(tr: Transaction): boolean {
  let found = false;
  tr.changes.iterChanges((_fromA, _toA, _fromB, _toB, inserted) => {
    if (inserted.lines > 1) found = true;
  });
  return found;
}

class SyncError extends Error {}

/** Next animation frame, with a timer fallback (frames pause while the window is hidden). */
function nextFrame(flush: () => void): void {
  let done = false;
  const run = () => {
    if (!done) {
      done = true;
      flush();
    }
  };
  requestAnimationFrame(run);
  setTimeout(run, 100);
}

export class Sync {
  private backend: Backend;
  private editor: Editor;
  private schedule: (flush: () => void) => void;
  private checkHash: boolean;
  private onError: (message: string) => void;

  private pending: ChangeSet;
  /** Start a new undo step with the next batch / the one after it. */
  private split = false;
  private splitAfter = false;
  private queue: Array<() => Promise<void>> = [];
  private running: Promise<void> | null = null;
  private flushScheduled = false;
  private suspended = false;
  private disposed = false;
  private lastHashCheck = -Infinity;

  constructor(backend: Backend, editor: Editor, options: SyncOptions = {}) {
    this.backend = backend;
    this.editor = editor;
    this.schedule = options.schedule ?? nextFrame;
    this.checkHash = options.checkHash ?? false;
    this.onError = options.onError ?? (() => {});
    this.pending = ChangeSet.empty(editor.state.doc.length);
  }

  /**
   * Feed every editor transaction here (e.g. from an update listener), also
   * selection-only ones: a cursor move by the user ends the undo step.
   */
  observe(tr: Transaction): void {
    if (this.disposed || tr.annotation(fromCore)) return;
    if (!tr.docChanged) {
      if (tr.selection && tr.isUserEvent("select")) this.requestSplit();
      return;
    }
    const ownStep = OWN_STEP.some((event) => tr.isUserEvent(event));
    if (ownStep) this.requestSplit();
    this.pending = this.pending.compose(tr.changes);
    // After a line break, the next paragraph starts a new step.
    if (ownStep || insertsLineBreak(tr)) this.requestSplit();
    if (!this.flushScheduled) {
      this.flushScheduled = true;
      this.schedule(() => {
        this.flushScheduled = false;
        void this.pump();
      });
    }
  }

  /**
   * Ends the current undo step: edits from now on start a new one. Edits
   * already waiting for this frame's batch stay in the old step (±1 frame).
   */
  requestSplit(): void {
    if (this.pending.empty) this.split = true;
    else this.splitAfter = true;
  }

  undo(): Promise<void> {
    return this.enqueue(() => this.coreOp(() => this.backend.undo()));
  }

  redo(): Promise<void> {
    return this.enqueue(() => this.coreOp(() => this.backend.redo()));
  }

  restore(id: number): Promise<void> {
    return this.enqueue(() => this.coreOp(() => this.backend.restore(id)));
  }

  /**
   * Another core operation that changes the text (e.g. replacing an image),
   * queued and applied like restore. Resolves to its reply (null once
   * disposed); its own errors reach the caller (nothing changed then).
   */
  async apply<R extends Reply>(request: () => Promise<R>): Promise<R | null> {
    let reply: R | null = null;
    let failed = false;
    let error: unknown;
    await this.enqueue(() =>
      this.coreOp(async () => {
        try {
          reply = await request();
        } catch (e) {
          failed = true;
          error = e;
        }
        return reply;
      }),
    );
    if (failed) throw error;
    return reply;
  }

  /**
   * A core request that leaves the text alone but reads or takes its
   * offsets (the review's), queued like `apply`: it runs once every earlier
   * edit is sent and nothing is in flight, so `toCore` and `toEditor` map
   * exactly while it runs and until it answers. Resolves to its result (null
   * once disposed); its own errors reach the caller.
   */
  async call<T>(request: () => Promise<T>): Promise<T | null> {
    let result: T | null = null;
    let failed = false;
    let error: unknown;
    await this.enqueue(async () => {
      await this.sendPending();
      try {
        result = await request();
      } catch (e) {
        failed = true;
        error = e;
      }
    });
    if (failed) throw error;
    return result;
  }

  /** Resolves once every local edit and queued operation reached the server. */
  async drain(): Promise<void> {
    while (!this.disposed && !this.suspended && (this.running || this.queue.length > 0 || !this.pending.empty)) {
      await this.pump();
    }
  }

  /** Drains, then sends nothing until `resume()` (local edits keep queuing). */
  async suspend(): Promise<void> {
    await this.drain();
    this.suspended = true;
  }

  resume(): void {
    this.suspended = false;
    void this.pump();
  }

  /** An offset into the server's text, mapped to the editor's (over the local edits not sent yet). */
  toEditor(pos: number): number {
    return this.pending.mapPos(Math.min(pos, this.pending.length));
  }

  /** An offset into the editor's text, mapped to the server's. */
  toCore(pos: number): number {
    return this.pending.invertedDesc.mapPos(Math.min(pos, this.pending.newLength));
  }

  /** Detaches from the editor; nothing is sent anymore. */
  dispose(): void {
    this.disposed = true;
    this.queue = [];
  }

  private enqueue(op: () => Promise<void>): Promise<void> {
    if (this.disposed) return Promise.resolve();
    return new Promise((resolve) => {
      this.queue.push(() => this.guard(op).finally(resolve));
      void this.pump();
    });
  }

  /** Runs queued operations, then pending edits, one request at a time. */
  private pump(): Promise<void> {
    if (this.running) return this.running;
    if (this.suspended || this.disposed || (this.queue.length === 0 && this.pending.empty)) {
      return Promise.resolve();
    }
    this.running = (async () => {
      while (!this.suspended && !this.disposed) {
        const op = this.queue.shift();
        if (op) await op();
        else if (!this.pending.empty) await this.guard(() => this.sendPending());
        else break;
      }
      this.running = null;
    })();
    return this.running;
  }

  private async guard(op: () => Promise<void>): Promise<void> {
    try {
      await op();
    } catch (e) {
      await this.resync(e);
    }
  }

  private async sendPending(): Promise<void> {
    if (this.pending.empty) return;
    const changes: TextChange[] = [];
    this.pending.iterChanges((from, to, _fromB, _toB, inserted) => {
      changes.push({ from, to, insert: inserted.toString() });
    });
    const expected = this.pending.newLength;
    const split = this.split;
    this.split = this.splitAfter;
    this.splitAfter = false;
    // Edits typed while this batch is in flight compose onto a fresh set.
    this.pending = ChangeSet.empty(expected);
    const len16 = await this.backend.edit(changes, split);
    if (len16 !== expected) throw new SyncError(`server length ${len16} after edit, expected ${expected}`);
    await this.verify();
  }

  /** Undo, redo, restore or `apply`: send pending edits first, then apply the reply. */
  private async coreOp(request: () => Promise<Reply | null>): Promise<void> {
    await this.sendPending();
    const reply = await request();
    if (reply) {
      // `pending.length` is still the server length before the operation.
      const remote = ChangeSet.of(reply.changes, this.pending.length);
      if (remote.newLength !== reply.len16) {
        throw new SyncError(`server length ${reply.len16}, changes give ${remote.newLength}`);
      }
      const local = remote.map(this.pending);
      this.pending = this.pending.map(remote, true);
      if (!local.empty) this.editor.dispatch({ changes: local, annotations: fromCore.of(true) });
    }
    await this.verify();
  }

  /** Development check: compare text hashes when nothing is pending. */
  private async verify(): Promise<void> {
    if (!this.checkHash || !this.pending.empty) return;
    const now = Date.now();
    if (now - this.lastHashCheck < 1000) return;
    this.lastHashCheck = now;
    const server = await this.backend.hash();
    if (!this.pending.empty) return; // typed meanwhile; check again later
    const local = textHash(this.editor.state.doc);
    if (server !== local) throw new SyncError(`text hash mismatch (server ${server}, editor ${local})`);
  }

  /** Replaces the editor text with the server's; local edits not yet sent are dropped. */
  private async resync(cause: unknown): Promise<void> {
    const reason = cause instanceof Error ? cause.message : String(cause);
    console.error("Lethora: editor out of sync, reloading the text:", cause);
    this.onError(`Editor resynced with the project (${reason}).`);
    try {
      const text = await this.backend.text();
      const doc = this.editor.state.doc.toString();
      let start = 0;
      const max = Math.min(doc.length, text.length);
      while (start < max && doc.charCodeAt(start) === text.charCodeAt(start)) start++;
      let end = 0;
      while (end < max - start && doc.charCodeAt(doc.length - 1 - end) === text.charCodeAt(text.length - 1 - end)) end++;
      if (start < doc.length - end || start < text.length - end) {
        this.editor.dispatch({
          changes: { from: start, to: doc.length - end, insert: text.slice(start, text.length - end) },
          annotations: fromCore.of(true),
        });
      }
    } catch (e) {
      console.error("Lethora: resync failed:", e);
      this.onError(`Could not reload the text: ${e instanceof Error ? e.message : String(e)}`);
    }
    this.pending = ChangeSet.empty(this.editor.state.doc.length);
    this.split = true;
    this.splitAfter = false;
  }
}
