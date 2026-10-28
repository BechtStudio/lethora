// Sync in the web view, the pure parts: the sync status as the core reports
// it (folder sync and server sync), the status cloud's look and tooltip,
// invite links and server addresses, and the pull schedule (one pull every
// 2 s and at once on `remote-changes`, never two at once).
//
// No Tauri imports and no DOM: this runs under `node --test`.

import type { Reply } from "./sync.ts";

/** How often other devices' changes are pulled. */
export const PULL_INTERVAL_MS = 2000;

export type SyncMode = "off" | "encrypted" | "plain" | "server";
/** Folder sync: ok, unavailable, needsCode, needsUpdate, error. Server sync: online, connecting, offline, removed, needsCode, needsUpdate, error. */
export type SyncState = "ok" | "unavailable" | "needsCode" | "needsUpdate" | "error" | "online" | "connecting" | "offline" | "removed";
export type Role = "owner" | "editor" | "reviewer" | "reader";

/** `sync_status`: encrypted sync, plain sync (a sync client's folder or other devices' files), server sync, or off. */
export interface SyncStatus {
  mode: SyncMode;
  /** The sync folder (encrypted sync). */
  folder?: string;
  state: SyncState;
  message?: string;
  /** Unix ms of the last successful push or pull. */
  lastSync?: number;
  /** The sync client whose folder holds the project. */
  provider?: string;
  /** The one-time notice about plain-text sync (until dismissed). */
  notice?: { provider: string };
  /** Server sync: the server's address. */
  server?: string;
  /** Server sync: this device's role. */
  role?: Role;
  /** Server sync while offline: Unix ms of the next try. */
  retryAt?: number;
}

/** `sync_pull`: the text changes (applied like a restore's) and what else to reload (flags only sent when true). */
export interface PullReply extends Reply {
  changed?: boolean;
  settings?: boolean;
  bibliography?: boolean;
  checkpoints?: boolean;
  /** Review items arrived or changed. */
  review?: boolean;
  status: SyncStatus;
}

/** The status cloud's look. */
export type CloudKind = "off" | "ok" | "plain" | "connecting" | "warning";

export function cloudKind(s: SyncStatus): CloudKind {
  if (s.mode === "off") return "off";
  if (s.mode === "plain") return "plain";
  if (s.mode === "server") return s.state === "online" ? "ok" : s.state === "connecting" ? "connecting" : "warning";
  return s.state === "ok" ? "ok" : "warning";
}

const hhmm = (ms: number) => new Date(ms).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });

const FALLBACK: Record<SyncState, string> = {
  ok: "Synced (encrypted)",
  unavailable: "The sync folder is not available.",
  needsCode: "Enter the project code to sync this device.",
  needsUpdate: "This project needs a newer Lethora.",
  error: "Sync has a problem.",
  online: "Synced through the server (encrypted)",
  connecting: "Connecting to the server…",
  offline: "The server can't be reached.",
  removed: "This device was removed from the project.",
};

const hhmmss = (ms: number) => new Date(ms).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", second: "2-digit" });

/** The host (and port) of server address `url`. */
export function serverHost(url: string): string {
  return url.replace(/^[a-z]+:\/\//i, "").replace(/\/.*$/, "");
}

/** The status cloud's tooltip for server sync. */
function serverTitle(s: SyncStatus, time: (ms: number) => string, timeSecs: (ms: number) => string): string {
  const host = s.server ? serverHost(s.server) : "the server";
  switch (s.state) {
    case "online":
      return s.lastSync ? `Synced with ${host} (encrypted) — last sync ${time(s.lastSync)}` : `Synced with ${host} (encrypted)`;
    case "connecting":
      return `Connecting to ${host}…`;
    case "offline":
      return `Offline: ${s.message ?? FALLBACK.offline}${s.retryAt ? ` Trying again at ${timeSecs(s.retryAt)}.` : ""} Your work is saved on this device.`;
    default:
      return s.message ?? FALLBACK[s.state];
  }
}

/** The status cloud's tooltip. */
export function cloudTitle(s: SyncStatus, time: (ms: number) => string = hhmm, timeSecs: (ms: number) => string = hhmmss): string {
  if (s.mode === "server") return serverTitle(s, time, timeSecs);
  switch (cloudKind(s)) {
    case "off":
      return "Sync is off. Click to share or sync this project.";
    case "plain":
      return s.provider ? `Synced in plain text by ${s.provider}` : "Other devices' changes arrive live (plain text)";
    case "ok":
      return s.lastSync ? `Synced (encrypted) — last sync ${time(s.lastSync)}` : FALLBACK.ok;
    case "warning":
    case "connecting":
      return s.message ?? FALLBACK[s.state];
  }
}

/** Whether `text` (as pasted) looks like an invite link rather than a project code. */
export function isInviteLink(text: string): boolean {
  return /^\s*(https?|wss?):\/\/[^\s/]+\/join\//i.test(text);
}

/** Whether server address `url` is plain http (allowed only on this machine and the local network). */
export function isPlainHttp(url: string): boolean {
  return /^\s*(http|ws):\/\//i.test(url);
}

/** The role that makes the text and its settings (`main`) read only on this device (reviewers and readers), or null. */
export function readOnlyRole(s: SyncStatus | null): Role | null {
  return s?.mode === "server" && (s.role === "reviewer" || s.role === "reader") ? s.role : null;
}

/** Whether the review (comments and highlights) is read only on this device: for readers. */
export function reviewReadOnly(s: SyncStatus | null): boolean {
  return s?.mode === "server" && s.role === "reader";
}

/** `server_members`: a member, with its current devices. */
export interface MemberInfo {
  user: string;
  name?: string;
  colour: string;
  role: Role;
  removed: boolean;
  devices: string[];
}

/** `server_members`: an invite that can still be used (no `role`: one for another device of its creator's user). */
export interface InviteInfo {
  id: string;
  creator: string;
  creator_user: string;
  role?: Exclude<Role, "owner">;
  /** Unix seconds. */
  expires: number;
  uses_left: number;
}

/** `server_members`: this device's user and device, the project's title (to confirm a deletion), who is in it and the colours to pick from. */
export interface MembersReply {
  user: string;
  device: string;
  title: string;
  members: MemberInfo[];
  invites: InviteInfo[];
  /** The colours a member may pick for themselves. */
  palette: string[];
}

/** Names of the profile colours (the core's `PALETTE`), for screen readers. */
const COLOUR_NAMES: Record<string, string> = {
  "#0072b2": "Blue",
  "#d55e00": "Vermilion",
  "#009e73": "Green",
  "#cc79a7": "Pink",
  "#e69f00": "Orange",
  "#56b4e9": "Sky blue",
  "#8e6c8a": "Mauve",
  "#a6761d": "Brown",
};

/** The name of profile colour `colour` of `palette`, e.g. "Blue" (an unknown one: "Colour 3"). */
export function colourName(colour: string, palette: readonly string[]): string {
  return COLOUR_NAMES[colour.toLowerCase()] ?? `Colour ${palette.indexOf(colour) + 1}`;
}

/** `server_remembered`: a project shared or joined here before whose local copy is gone. */
export interface RememberedProject {
  project: string;
  server: string;
  title: string;
}

/** A member's name: from its profile, else a short id. */
export function memberName(m: Pick<MemberInfo, "user" | "name">): string {
  return m.name?.trim() || `Member ${m.user.slice(0, 6)}`;
}

/** A device in the members list: "This device" for `own`, else a short id. */
export function deviceName(device: string, own: string): string {
  return device === own ? "This device" : `Device ${device.slice(0, 6)}`;
}

/** What an invite is for and how long it lasts, e.g. "Editor · by Alice · until 25 Oct 2026 · 1 use left". */
export function inviteSummary(i: InviteInfo, members: readonly MemberInfo[], date: (ms: number) => string): string {
  const creator = members.find((m) => m.user === i.creator_user);
  const kind = i.role ? `${i.role[0].toUpperCase()}${i.role.slice(1)}` : "Device";
  const uses = i.uses_left === 1 ? "1 use left" : `${i.uses_left} uses left`;
  return `${kind} · by ${creator ? memberName(creator) : "a former member"} · until ${date(i.expires * 1000)} · ${uses}`;
}

/** The roles an invite can give, with their labels. */
export const INVITE_ROLES: ReadonlyArray<{ role: Exclude<Role, "owner">; label: string }> = [
  { role: "editor", label: "Editor (writes text and layout)" },
  { role: "reviewer", label: "Reviewer (comments and highlights, doesn't edit the text)" },
  { role: "reader", label: "Reader (reads only)" },
];
export const INVITE_DAYS = [1, 7, 30] as const;
export const INVITE_USES = [1, 5, 25] as const;

/** `sync_apps`: a sync app's folder on this device. */
export interface SyncApp {
  id: string;
  label: string;
  path: string;
}

/** `sync_find`: the synced project a code unlocks, and a project on this device syncing with it already. */
export interface FindReply {
  replica?: string;
  /** The sync apps searched (none when a folder was given). */
  searched: string[];
  local?: string;
}

/** "a", "a and b", "a, b and c". */
function listed(items: string[]): string {
  return items.length < 2 ? (items[0] ?? "") : `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

/** Why nothing was found for a code: in `folder` if one was chosen, else in the sync apps `searched`. */
export function notFoundMessage(searched: string[], folder: string | null): string {
  if (folder !== null) return `No synced project with this code is in ${folder}.`;
  if (searched.length === 0) return "No sync app (Dropbox, OneDrive, iCloud Drive, Google Drive) was found on this device.";
  return `No synced project with this code was found in ${listed(searched)}.`;
}

/** The replica folder's name without `.lethora-sync`: a name for the joined project. */
export function joinedName(replica: string): string {
  const name = replica.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? "";
  return name.replace(/\.lethora-sync$/i, "") || "Synced project";
}

export interface Timers {
  every(f: () => void, ms: number): unknown;
  cancel(id: unknown): void;
}

const globalTimers: Timers = {
  every: (f, ms) => setInterval(f, ms),
  cancel: (id) => clearInterval(id as ReturnType<typeof setInterval>),
};

/**
 * Runs `run` every interval while started, never twice at once: a tick is
 * skipped while the previous run is still queued or in flight.
 */
export class Poller {
  private run: () => Promise<unknown>;
  private intervalMs: number;
  private timers: Timers;
  private onError: (e: unknown) => void;
  private timer: unknown = null;
  private busy = false;
  private again = false;
  private stopped = false;

  constructor(run: () => Promise<unknown>, options: { intervalMs?: number; timers?: Timers; onError?: (e: unknown) => void } = {}) {
    this.run = run;
    this.intervalMs = options.intervalMs ?? PULL_INTERVAL_MS;
    this.timers = options.timers ?? globalTimers;
    this.onError = options.onError ?? (() => {});
  }

  start(): void {
    if (this.timer === null && !this.stopped) this.timer = this.timers.every(() => void this.tick(), this.intervalMs);
  }

  /** For good: no more runs (one in flight finishes). */
  stop(): void {
    this.stopped = true;
    if (this.timer !== null) this.timers.cancel(this.timer);
    this.timer = null;
  }

  /** Whether a run is queued or in flight. */
  get running(): boolean {
    return this.busy;
  }

  /** Runs once unless a run is still going or stopped; returns whether it ran. */
  tick(): boolean {
    if (this.busy || this.stopped) return false;
    this.busy = true;
    void this.run()
      .catch(this.onError)
      .finally(() => {
        this.busy = false;
        if (this.again) {
          this.again = false;
          this.tick();
        }
      });
    return true;
  }

  /** Runs now, or once more right after the run in flight (news arrived meanwhile, e.g. `remote-changes`). */
  request(): void {
    if (this.stopped) return;
    if (!this.tick()) this.again = true;
  }
}
