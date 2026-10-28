// Help → About Lethora: the app's version and build, the protocol and the
// feature level it speaks, the OS, and the connected server's version, with
// "Copy diagnostic info" (the same as plain text, for bug reports).

/** `app_info`. */
export interface AppInfo {
  /** SemVer, from the workspace `Cargo.toml`. */
  version: string;
  /** The short git hash of the build, `-dirty` with uncommitted changes, or "unknown". */
  git: string;
  protocol: number;
  level: number;
  os: string;
  arch: string;
  /** The open project's server's version, once it said so. */
  server?: string;
}

/** The facts the dialog lists, label and value. */
export function aboutFacts(info: AppInfo): Array<[string, string]> {
  return [
    ["Version", `${info.version} (${info.git})`],
    ["Protocol", String(info.protocol)],
    ["Feature level", String(info.level)],
    ["System", info.arch ? `${info.os} ${info.arch}` : info.os],
    ["Server", info.server ?? "Not connected"],
  ];
}

/** The diagnostic info as plain text, one fact per line. */
export function diagnosticText(info: AppInfo): string {
  return ["Lethora", ...aboutFacts(info).map(([label, value]) => `${label}: ${value}`)].join("\n") + "\n";
}

const byId = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

function el<K extends keyof HTMLElementTagNameMap>(tag: K, props: Partial<HTMLElementTagNameMap[K]> = {}, ...children: Array<Node | string>): HTMLElementTagNameMap[K] {
  const e = Object.assign(document.createElement(tag), props);
  e.append(...children);
  return e;
}

export class AboutDialog {
  private dialog = byId<HTMLDialogElement>("about-dialog");
  private body = byId("about-body");

  open(info: AppInfo): void {
    const facts = el("dl", { className: "sd-facts" });
    for (const [label, value] of aboutFacts(info)) facts.append(el("dt", { textContent: label }), el("dd", { textContent: value, title: value }));
    const note = el("p", { className: "sd-note", role: "status" });
    const copy = el("button", { type: "button", textContent: "Copy diagnostic info" });
    copy.addEventListener("click", () => {
      navigator.clipboard.writeText(diagnosticText(info)).then(
        () => (note.textContent = "Copied. Paste it into your bug report."),
        () => (note.textContent = "Could not copy. Select the lines above and copy them by hand."),
      );
    });
    const close = el("button", { type: "button", className: "primary", textContent: "Close" });
    close.addEventListener("click", () => this.dialog.close());
    this.body.replaceChildren(el("h2", { id: "about-heading", textContent: "About Lethora" }), facts, note, el("div", { className: "np-actions" }, copy, close));
    if (!this.dialog.open) this.dialog.showModal();
    close.focus();
  }
}
