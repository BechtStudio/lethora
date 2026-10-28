// Live presence in a server-shared project: the other members' carets and
// selections in the author and science views, and the title bar's row of who
// is online. The core publishes this device's view and selection and lists
// the others' (`presence_set`, `presence_get`; crates/core/src/server.rs,
// "Presence"), positions as offsets into the core's text; the edit queue
// maps them to the editor's (see `Sync.toEditor`).
//
// No Tauri imports: this runs under `node --test`.

import { type Extension, type Range, StateEffect, StateField } from "@codemirror/state";
import { Decoration, type DecorationSet, EditorView, WidgetType } from "@codemirror/view";
import type { ViewMode } from "./chrome.ts";
import type { Role } from "./pull.ts";

/** How often this device's presence goes to the core at most (the core sends it on as often). */
export const PRESENCE_GAP_MS = 100;

/** `presence_get`: another device's presence (offsets into the core's text). */
export interface PeerPresence {
  device: string;
  user: string;
  name?: string;
  colour: string;
  role: Role;
  view: ViewMode;
  /** Missing where the core can't place it (text not pulled yet). */
  anchor?: number;
  head?: number;
}

/** A remote caret and selection as the editor shows it. */
export interface RemoteCaret {
  device: string;
  name: string;
  colour: string;
  anchor: number;
  head: number;
}

/** Shown for a member whose profile doesn't open (yet). */
const UNNAMED = "Someone";
/** For a colour that isn't `#rrggbb`. */
const FALLBACK_COLOUR = "#8e6c8a";
const ROLES: Record<Role, string> = { owner: "Owner", editor: "Editor", reviewer: "Reviewer", reader: "Reader" };
const VIEWS: readonly ViewMode[] = ["author", "science", "publisher"];

export const displayName = (p: { name?: string }): string => p.name?.trim() || UNNAMED;

/** `colour` if it is `#rrggbb` (it goes into styles), else a neutral one. */
export const safeColour = (colour: string): string => (/^#[0-9a-f]{6}$/i.test(colour) ? colour : FALLBACK_COLOUR);

/** Readable text on `colour` (`#rrggbb`): white or near-black, whichever contrasts more. */
export function inkOn(colour: string): string {
  const channel = (i: number) => {
    const c = parseInt(colour.slice(i, i + 2), 16) / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  const l = 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5);
  // Contrast with white (1.05 / (l + 0.05)) against with #1d1c1a (l ≈ 0.012).
  return 1.05 / (l + 0.05) >= (l + 0.05) / 0.062 ? "#ffffff" : "#1d1c1a";
}

/** One or two letters for `name`: the first word's and the last word's first. */
export function initials(name: string): string {
  const words = name.trim().split(/\s+/).filter(Boolean);
  if (words.length === 0) return "?";
  const first = (w: string) => Array.from(w)[0].toLocaleUpperCase();
  return words.length === 1 ? first(words[0]) : first(words[0]) + first(words[words.length - 1]);
}

/**
 * The carets to show: peers in the author or science view whose selection
 * the core placed, mapped by `toEditor` (core → editor offsets) and kept
 * within `length`.
 */
export function caretsOf(peers: readonly PeerPresence[], toEditor: (pos: number) => number, length: number): RemoteCaret[] {
  const place = (pos: number) => Math.max(0, Math.min(length, toEditor(pos)));
  return peers
    .filter((p) => p.view !== "publisher" && p.anchor !== undefined && p.head !== undefined)
    .map((p) => ({ device: p.device, name: displayName(p), colour: safeColour(p.colour), anchor: place(p.anchor!), head: place(p.head!) }));
}

/** A member in the title bar's row. */
export interface OnlineUser {
  user: string;
  initials: string;
  colour: string;
  /** Text on `colour`. */
  ink: string;
  /** Name, role and view(s), for the tooltip. */
  title: string;
}

function viewsText(views: readonly ViewMode[]): string {
  if (views.length === 1) return `${views[0][0].toUpperCase()}${views[0].slice(1)} view`;
  const list = views.slice(0, -1).join(", ") + ` and ${views[views.length - 1]}`;
  return `${list[0].toUpperCase()}${list.slice(1)} views`;
}

/** How many users the online row shows before a "+N" for the rest. */
export const ONLINE_SHOWN = 5;

/** The online row as shown: up to `max` users, the rest behind a "+N". */
export function onlineRow(users: readonly OnlineUser[], max = ONLINE_SHOWN): { shown: OnlineUser[]; rest: OnlineUser[] } {
  return users.length <= max ? { shown: [...users], rest: [] } : { shown: users.slice(0, max), rest: users.slice(max) };
}

/** Who is online, each user once (with the views of all their devices), by name. */
export function onlineUsers(peers: readonly PeerPresence[]): OnlineUser[] {
  const users = new Map<string, { peer: PeerPresence; views: Set<ViewMode> }>();
  for (const p of peers) {
    const seen = users.get(p.user);
    if (seen) seen.views.add(p.view);
    else users.set(p.user, { peer: p, views: new Set([p.view]) });
  }
  return [...users.values()]
    .map(({ peer, views }) => {
      const name = displayName(peer);
      const shown = VIEWS.filter((v) => views.has(v));
      const colour = safeColour(peer.colour);
      return { name, user: peer.user, initials: initials(name), colour, ink: inkOn(colour), title: `${name} · ${ROLES[peer.role] ?? peer.role} · ${viewsText(shown)}` };
    })
    .sort((a, b) => a.name.localeCompare(b.name) || a.user.localeCompare(b.user))
    .map(({ name: _name, ...user }) => user);
}

// ---- The editor's remote carets ----

const setCarets = StateEffect.define<RemoteCaret[]>();

/** Replaces the remote carets (positions in the editor's current text). */
export const setRemoteCarets = (carets: RemoteCaret[]): StateEffect<RemoteCaret[]> => setCarets.of(carets);

/** A caret as shown: `moves` counts its moves, so its name flag shows again after each. */
export interface ShownCaret extends RemoteCaret {
  moves: number;
}

class CaretWidget extends WidgetType {
  readonly name: string;
  readonly colour: string;
  readonly moves: number;

  constructor(name: string, colour: string, moves: number) {
    super();
    this.name = name;
    this.colour = colour;
    this.moves = moves;
  }

  eq(other: CaretWidget): boolean {
    return other.name === this.name && other.colour === this.colour && other.moves === this.moves;
  }

  toDOM(): HTMLElement {
    const caret = document.createElement("span");
    caret.className = "cm-remote-caret";
    caret.style.setProperty("--peer", this.colour);
    caret.style.setProperty("--peer-ink", inkOn(this.colour));
    caret.setAttribute("aria-hidden", "true");
    const flag = document.createElement("span");
    flag.className = "cm-remote-flag";
    flag.textContent = this.name;
    // Word joiners give the caret the line's height without breaking it.
    caret.append("⁠", flag, "⁠");
    return caret;
  }
}

function decorations(carets: readonly ShownCaret[]): DecorationSet {
  const ranges: Range<Decoration>[] = [];
  for (const c of carets) {
    const from = Math.min(c.anchor, c.head);
    const to = Math.max(c.anchor, c.head);
    if (from < to) {
      ranges.push(Decoration.mark({ class: "cm-remote-selection", attributes: { style: `--peer: ${c.colour}` } }).range(from, to));
    }
    ranges.push(Decoration.widget({ widget: new CaretWidget(c.name, c.colour, c.moves) }).range(c.head));
  }
  return Decoration.set(ranges, true);
}

/** The remote carets: mapped over edits, replaced by `setRemoteCarets`. */
export const remoteCaretField = StateField.define<readonly ShownCaret[]>({
  create: () => [],
  update(shown, tr) {
    let next = tr.docChanged ? shown.map((c) => ({ ...c, anchor: tr.changes.mapPos(c.anchor), head: tr.changes.mapPos(c.head) })) : shown;
    for (const effect of tr.effects) {
      if (!effect.is(setCarets)) continue;
      const before = next;
      next = effect.value.map((c) => {
        const was = before.find((b) => b.device === c.device);
        return { ...c, moves: !was ? 1 : was.head !== c.head ? was.moves + 1 : was.moves };
      });
    }
    return next;
  },
  provide: (field) => EditorView.decorations.from(field, decorations),
});

/** Remote carets and selections in the editor (empty until `setRemoteCarets`). */
export const remoteCarets: Extension = remoteCaretField;

// ---- This device's presence ----

/** Where this device's user is: the view and the selection (core offsets). */
export interface Here {
  view: ViewMode;
  anchor: number;
  head: number;
}

const same = (a: Here | null, b: Here | null) => a !== null && b !== null && a.view === b.view && a.anchor === b.anchor && a.head === b.head;

/** Sends this device's presence at most every `gap` ms (the latest wins), nothing unchanged. */
export class PresenceSender {
  private last: Here | null = null;
  private next: Here | null = null;
  private waiting = false;
  private sentAt = -Infinity;
  private send: (here: Here) => Promise<void>;
  private gap: number;
  private now: () => number;
  private later: (run: () => void, ms: number) => void;

  constructor(
    send: (here: Here) => Promise<void>,
    gap = PRESENCE_GAP_MS,
    now: () => number = () => Date.now(),
    later: (run: () => void, ms: number) => void = (run, ms) => void setTimeout(run, ms),
  ) {
    this.send = send;
    this.gap = gap;
    this.now = now;
    this.later = later;
  }

  update(here: Here): void {
    this.next = here;
    if (this.waiting) return;
    const wait = this.sentAt + this.gap - this.now();
    if (wait <= 0) {
      this.flush();
      return;
    }
    this.waiting = true;
    this.later(() => {
      this.waiting = false;
      this.flush();
    }, wait);
  }

  /** Forgets what was sent (another project): the next update goes out. */
  reset(): void {
    this.last = null;
  }

  private flush(): void {
    const here = this.next;
    this.next = null;
    if (!here || same(here, this.last)) return;
    this.last = here;
    this.sentAt = this.now();
    // Sent again with the next change if this one failed.
    this.send(here).catch(() => (this.last = null));
  }
}
