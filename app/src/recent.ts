// Recent projects: File → Open recent and the start screen's list. The core
// keeps the list (`projects_recent`: newest first, at most 10, without the
// open project); a project gone from disk is dropped when opening it fails.

import type { Command } from "./commands.ts";

/** A recent project as `projects_recent` lists it. */
export interface RecentProject {
  name: string;
  path: string;
  /** Its folder is still there. */
  exists: boolean;
}

/** Entry `i` (0-based) of Open recent: mnemonics 1–9, then 0 (a `&` in the name stays text). */
export const recentLabel = (i: number, name: string): string => `&${(i + 1) % 10} ${name}`;

/** Open recent's submenu: the projects (path beside), then Clear recent; empty without projects. */
export function recentItems(list: readonly RecentProject[], open: (path: string) => unknown, clear: () => unknown): Array<Command | null> {
  if (!list.length) return [];
  return [
    ...list.slice(0, 10).map((p, i): Command => ({ id: `recent-${i}`, label: recentLabel(i, p.name), detail: p.path, run: () => open(p.path) })),
    null,
    { id: "recent-clear", label: "&Clear recent", run: clear },
  ];
}

interface RecentHost {
  open: (path: string) => void;
  /** Takes a project off the list; resolves to the new list. */
  remove: (path: string) => Promise<readonly RecentProject[]>;
  error: (e: unknown) => void;
}

/**
 * The start screen's list: click or Enter opens, × or Delete takes a project
 * off the list (not off the disk), Up/Down move. No heading when empty.
 */
export class RecentList {
  private section = document.getElementById("recent")!;
  private list = document.getElementById("recent-list")!;
  private host: RecentHost;

  constructor(host: RecentHost) {
    this.host = host;
    this.list.addEventListener("keydown", (e) => {
      const buttons = [...this.list.querySelectorAll<HTMLElement>(".recent-open")];
      const at = buttons.indexOf(e.target as HTMLElement);
      if (at < 0 || e.ctrlKey || e.metaKey || e.altKey) return;
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        buttons[(at + (e.key === "ArrowDown" ? 1 : -1) + buttons.length) % buttons.length].focus();
      } else if (e.key === "Delete") {
        e.preventDefault();
        this.remove(buttons[at].dataset.path!, at);
      }
    });
  }

  set(list: readonly RecentProject[]): void {
    this.section.hidden = list.length === 0;
    this.list.replaceChildren(
      ...list.map((p, i) => {
        const open = document.createElement("button");
        open.type = "button";
        open.className = "recent-open";
        open.classList.toggle("missing", !p.exists);
        open.dataset.path = p.path;
        open.title = p.exists ? p.path : `${p.path} (not found)`;
        const name = document.createElement("span");
        name.className = "recent-name";
        name.textContent = p.name;
        const path = document.createElement("span");
        path.className = "recent-path";
        path.textContent = p.path;
        open.append(name, path);
        open.addEventListener("click", () => this.host.open(p.path));
        const remove = document.createElement("button");
        remove.type = "button";
        remove.className = "recent-remove";
        remove.tabIndex = -1;
        remove.textContent = "×";
        remove.title = "Remove from this list (the project stays on disk; Delete)";
        remove.setAttribute("aria-label", `Remove ${p.name} from the recent projects`);
        remove.addEventListener("click", () => this.remove(p.path, i));
        const item = document.createElement("li");
        item.append(open, remove);
        return item;
      }),
    );
  }

  /** Takes entry `at` off the list; the focus stays at that place if it was in the list. */
  private remove(path: string, at: number): void {
    const focused = this.list.contains(document.activeElement);
    this.host.remove(path).then((list) => {
      this.set(list);
      const buttons = this.list.querySelectorAll<HTMLElement>(".recent-open");
      if (focused) (buttons[Math.min(at, buttons.length - 1)] ?? document.getElementById("new"))?.focus();
    }, this.host.error);
  }
}
