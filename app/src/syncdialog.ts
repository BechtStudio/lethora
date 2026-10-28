// Sync UI: the status cloud in the top bar, the "Share / Sync" dialog
// (share on a Lethora server and invite people or another own device; or
// set up folder sync in a sync app's folder or another one, show the
// project code, enter it, stop), "Open shared project…" (paste an invite
// link, joined through the server, or the code, the project is found in the
// sync apps' folders and joined; projects shared or joined here before
// whose local copy is gone are offered to reopen from their server), the
// members screen (roles, removing, invites, leaving, deleting from the
// server) and the one-time notice for projects synced in plain text.
//
// No Tauri imports: commands come in through `SyncApi`.

import { cloudKind, cloudTitle, colourName, deviceName, INVITE_DAYS, INVITE_ROLES, INVITE_USES, inviteSummary, isInviteLink, isPlainHttp, memberName, notFoundMessage, serverHost, type CloudKind, type FindReply, type MemberInfo, type MembersReply, type RememberedProject, type Role, type SyncApp, type SyncStatus } from "./pull.ts";

export interface SyncApi {
  status(): Promise<SyncStatus>;
  /** Creates the encrypted copy inside `folder`; the new project code. */
  setup(folder: string): Promise<{ code: string; status: SyncStatus }>;
  /** Creates the encrypted copy in `Lethora/` in sync app `id`'s folder; the new project code. */
  setupApp(id: string): Promise<{ code: string; status: SyncStatus }>;
  /** The sync apps' folders on this device. */
  apps(): Promise<SyncApp[]>;
  code(): Promise<string>;
  enterCode(code: string): Promise<SyncStatus>;
  stop(): Promise<SyncStatus>;
  dismissNotice(): Promise<void>;
  /** Asks for the folder to keep the encrypted copy in (null if cancelled). */
  pickFolder(): Promise<string | null>;
  /** Finds the synced project `code` unlocks: in `folder` if given, else in the sync apps' folders. */
  find(code: string, folder: string | null): Promise<FindReply>;
  /** Asks for the folder holding a synced project (null if cancelled). */
  pickSharedFolder(): Promise<string | null>;
  /** The servers used last and the user's display name, to prefill the forms. */
  serverRecent(): Promise<{ servers: string[]; name: string | null }>;
  /** Shares the project on the Lethora server at `url`; resolves once it is there. */
  serverShare(url: string, name: string): Promise<SyncStatus>;
  /** An invite link: for a new member with `role`, or (null) for another device of this user. */
  serverInvite(role: Exclude<Role, "owner"> | null, days: number, uses: number): Promise<string>;
  /** Who is in the project, the open invites and the project's title. */
  serverMembers(): Promise<MembersReply>;
  /** Gives member `user` a new role (the owner only). */
  serverSetRole(user: string, role: Exclude<Role, "owner">): Promise<void>;
  /** Gives this user `colour` (one of the members reply's palette) in this project. */
  serverSetColour(colour: string): Promise<void>;
  /** Removes a member with its devices (the owner only), or one device. */
  serverRemove(target: { user: string } | { device: string }): Promise<void>;
  serverRevoke(invite: string): Promise<void>;
  /** Leaves the project with this device; the copy here turns into a local project. */
  serverLeave(): Promise<SyncStatus>;
  /** Deletes the project from the server (the owner only); local copies stay. */
  serverDelete(): Promise<SyncStatus>;
  /** Projects shared or joined here before whose local copy is gone. */
  serverRemembered(): Promise<RememberedProject[]>;
}

export interface SyncHost {
  api: SyncApi;
  /** A new status (after setup, stop, a code). */
  onStatus(status: SyncStatus): void;
  /** Joins the synced project in `replica` and opens it; rejects if that failed. */
  join(replica: string, code: string): Promise<void>;
  /** Joins the project of invite link `link` through its server and opens it; rejects if that failed. */
  joinServer(link: string, name: string): Promise<void>;
  /** Reopens remembered project `project` from its server and opens it; rejects if that failed. */
  reopenServer(project: string): Promise<void>;
  /** Opens the project at `path`; rejects if that failed. */
  openProject(path: string): Promise<void>;
  info(message: string): void;
  error(e: unknown): void;
}

const SVG_NS = "http://www.w3.org/2000/svg";
/** The same cloud outline as the assets panel's status clouds. */
const CLOUD = "M6.5 19a4.5 4.5 0 0 1-.4-9A6 6 0 0 1 17.6 8.5 5.25 5.25 0 0 1 17.5 19z";
const MARKS: Record<CloudKind, string[]> = {
  off: [],
  ok: ["M9 14l2 2 4-4.5"],
  // An open padlock.
  plain: ["M10 13.5h4v3.5h-4z", "M13 13.5v-1.6a1.5 1.5 0 0 1 3 0"],
  warning: ["M12 10.5v3.2", "M12 16.4v.1"],
  connecting: ["M9 14.5h.01", "M12 14.5h.01", "M15 14.5h.01"],
};

const ROLE_NAMES: Record<Role, string> = { owner: "Owner", editor: "Editor", reviewer: "Reviewer", reader: "Reader" };

const STATE_NAMES: Partial<Record<SyncStatus["state"], string>> = {
  online: "Online",
  connecting: "Connecting…",
  offline: "Offline (trying again)",
  removed: "Removed from the project",
  needsCode: "No key on this device",
  needsUpdate: "Needs a newer Lethora",
  error: "Stopped",
};

function select(id: string, options: ReadonlyArray<readonly [string, string]>, value: string): HTMLSelectElement {
  const s = el("select", { id, className: "sd-select" });
  for (const [v, label] of options) s.append(el("option", { value: v, textContent: label, selected: v === value }));
  return s;
}

const byId = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

function el<K extends keyof HTMLElementTagNameMap>(tag: K, props: Partial<HTMLElementTagNameMap[K]> = {}, ...children: Array<Node | string>): HTMLElementTagNameMap[K] {
  const e = Object.assign(document.createElement(tag), props);
  e.append(...children);
  return e;
}

const button = (textContent: string, onClick: () => void, className = "") => {
  const b = el("button", { type: "button", textContent, className });
  b.addEventListener("click", onClick);
  return b;
};

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

const dateTime = (ms: number) => new Date(ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });

/** The cloud button in the top bar. */
export class SyncCloud {
  private button = byId<HTMLButtonElement>("sync-cloud");

  constructor(onClick: () => void) {
    this.button.addEventListener("click", onClick);
  }

  set(status: SyncStatus | null): void {
    this.button.hidden = status === null;
    if (!status) return;
    const kind = cloudKind(status);
    const title = cloudTitle(status);
    this.button.dataset.kind = kind;
    this.button.title = title;
    this.button.setAttribute("aria-label", `Sync: ${title}`);
    const svg = document.createElementNS(SVG_NS, "svg");
    svg.setAttribute("viewBox", "0 0 24 24");
    svg.setAttribute("aria-hidden", "true");
    for (const d of [CLOUD, ...MARKS[kind]]) {
      const path = document.createElementNS(SVG_NS, "path");
      path.setAttribute("d", d);
      svg.append(path);
    }
    this.button.replaceChildren(svg);
  }
}

/** The sync dialog; its content follows the status. */
export class SyncDialog {
  private host: SyncHost;
  private dialog = byId<HTMLDialogElement>("sync-dialog");
  private body = byId("sd-body");

  constructor(host: SyncHost) {
    this.host = host;
  }

  /** Opens the dialog for `status`. */
  open(status: SyncStatus): void {
    this.render(status);
    if (!this.dialog.open) this.dialog.showModal();
    this.body.querySelector<HTMLElement>("[autofocus], .primary")?.focus();
  }

  /** Opens the dialog at the choice of where to keep the encrypted copy. */
  startSetup(status: SyncStatus): void {
    this.choices(status);
    if (!this.dialog.open) this.dialog.showModal();
  }

  /** "Open shared project…": paste the project code; the synced project it unlocks is found and joined. */
  openShared(): void {
    this.sharedScreen("", "", false);
    if (!this.dialog.open) this.dialog.showModal();
    this.body.querySelector<HTMLElement>(".sd-code-input")?.focus();
  }

  /** File → Members…: the members screen of a project shared on a server. */
  members(status: SyncStatus): void {
    if (status.mode !== "server") {
      this.open(status);
      return;
    }
    this.membersScreen(status);
    if (!this.dialog.open) this.dialog.showModal();
  }

  close(): void {
    if (this.dialog.open) this.dialog.close();
  }

  /** Replaces the dialog's content: a heading, the content, an error line and the action buttons. */
  private screen(heading: string, content: Node[], actions: HTMLButtonElement[]): void {
    const actionRow = el("div", { className: "np-actions sd-actions" }, ...actions);
    this.body.replaceChildren(el("h2", { id: "sd-heading", textContent: heading }), ...content, el("p", { className: "sd-error", role: "alert", hidden: true }), actionRow);
  }

  private showError(text: string): void {
    const error = this.body.querySelector<HTMLElement>(".sd-error")!;
    error.textContent = text;
    error.hidden = !text;
  }

  private closeButton(className = ""): HTMLButtonElement {
    return button("Close", () => this.close(), className);
  }

  private render(status: SyncStatus): void {
    const folder = status.folder ? [el("p", { className: "np-path", textContent: status.folder, title: status.folder })] : [];
    const stop = button("Stop syncing…", () => this.confirmStop());
    const setup = (className = "") => button("Set up encrypted sync…", () => this.choices(status), className);
    if (status.mode === "off") {
      this.choices(status);
    } else if (status.mode === "server") {
      this.serverScreen(status);
    } else if (status.mode === "plain") {
      const by = status.provider;
      this.screen(
        by ? `Synced by ${by}` : "Synced in plain text",
        [
          el("p", {
            textContent: by
              ? `This project's folder is synced by ${by} in plain text, so ${by} can read it. Changes from your other devices arrive live.`
              : "Other devices write to this project folder directly, in plain text. Their changes arrive live.",
          }),
          el("p", { textContent: "For privacy, move the project out of the synced folder and set up encrypted sync." }),
        ],
        [setup(), this.closeButton("primary")],
      );
    } else if (status.state === "ok") {
      const rows = el(
        "dl",
        { className: "sd-facts" },
        el("dt", { textContent: "Sync folder" }),
        el("dd", { textContent: status.folder ?? "", title: status.folder ?? "" }),
        el("dt", { textContent: "Last sync" }),
        el("dd", { textContent: status.lastSync ? dateTime(status.lastSync) : "Not yet" }),
      );
      const codeSlot = el("div", { className: "sd-code-slot" });
      const show = button("Show project code", () => void this.revealCode(codeSlot, show));
      const note = el("p", { className: "sd-note", textContent: "Sharing on a Lethora server needs folder sync off: one way at a time." });
      this.screen("Share / Sync", [rows, codeSlot, note], [stop, show, this.closeButton("primary")]);
    } else if (status.state === "needsCode") {
      const input = this.codeInput("");
      const form = this.codeForm(input, "Connect", (code) => void this.enterCode(code));
      this.screen("Enter the project code", [el("p", { textContent: status.message ?? "Enter the project code to sync this device." }), ...folder, form], [stop, this.closeButton()]);
      input.setAttribute("autofocus", "");
    } else {
      const update = status.state === "needsUpdate";
      const note = update ? "Your changes still go to the sync folder; the others' arrive again once Lethora is updated." : "Lethora keeps trying. Your work is saved in the project folder meanwhile.";
      this.screen(
        update ? "Update Lethora" : status.state === "unavailable" ? "Sync folder not available" : "Sync problem",
        [el("p", { textContent: status.message ?? "" }), ...folder, el("p", { className: "sd-note", textContent: note })],
        [stop, this.closeButton("primary")],
      );
    }
  }

  private codeInput(value: string): HTMLInputElement {
    return el("input", { type: "text", value, className: "sd-code-input", placeholder: "XXXX-XXXX-…", autocomplete: "off", spellcheck: false, ariaLabel: "Project code" });
  }

  /** The code field with its submit button (the dialog's primary action). */
  private codeForm(input: HTMLInputElement, label: string, submit: (code: string) => void): HTMLFormElement {
    const form = el("form", { className: "sd-code-form" }, input, el("button", { type: "submit", className: "primary", textContent: label }));
    form.addEventListener("submit", (e) => {
      e.preventDefault();
      const code = input.value.trim();
      if (code) submit(code);
      else this.showError("Enter the project code.");
    });
    return form;
  }

  /** The project code in a box with "Copy", and what to do with it. */
  private codeBox(code: string): Node[] {
    const copy = button("Copy", () => {
      navigator.clipboard.writeText(code).then(
        () => (copy.textContent = "Copied"),
        () => this.showError("Could not copy. Select the code and copy it by hand."),
      );
    });
    return [
      el("div", { className: "sd-code" }, el("code", { textContent: code }), copy),
      el("p", { className: "sd-note", textContent: "On your other device, choose Open shared project… and paste this code. Keep it somewhere safe: it is also the recovery key." }),
    ];
  }

  /** "Share / Sync" while sync is off: one button per sync app found on this device, then "Other folder…". */
  private choices(status: SyncStatus): void {
    const options = el("div", { className: "sd-apps" });
    const other = button("Other folder…", () => void this.setupInFolder(status), "sd-app");
    other.title = "Any folder your devices sync: Syncthing, a network drive, a USB stick";
    options.append(other);
    const openShared = button("Open shared project…", () => this.openShared());
    openShared.title = "Open another project shared from another device";
    this.screen(
      "Share / Sync",
      [
        el("h3", { className: "sd-section", textContent: "Lethora server" }),
        el("p", { textContent: "Share through a Lethora server: you invite people (and your other devices) with a link, and changes arrive live. Everything is encrypted on this device." }),
        ...this.serverForm(),
        el("h3", { className: "sd-section", textContent: "Folder" }),
        el("p", { textContent: "Or keep an encrypted copy in a folder your other devices sync. They open it with the project code." }),
        options,
      ],
      [openShared, this.closeButton()],
    );
    this.host.api.apps().then(
      (apps) => {
        if (!options.isConnected) return;
        const buttons = apps.map((app) => {
          const b = button(app.label, () => void this.setup(status, () => this.host.api.setupApp(app.id)), "sd-app");
          b.title = `Keep the copy in ${app.path}`;
          return b;
        });
        options.prepend(...buttons);
        (buttons[0] ?? other).focus();
      },
      (e) => this.showError(message(e)),
    );
  }

  /** "Other folder…": the copy goes into a new folder inside the one picked. */
  private async setupInFolder(status: SyncStatus): Promise<void> {
    let folder: string | null;
    try {
      folder = await this.host.api.pickFolder();
    } catch (e) {
      this.showError(message(e));
      return;
    }
    if (folder !== null) await this.setup(status, () => this.host.api.setup(folder));
  }

  private async setup(status: SyncStatus, start: () => Promise<{ code: string; status: SyncStatus }>): Promise<void> {
    this.body.querySelectorAll<HTMLButtonElement>(".sd-app").forEach((b) => (b.disabled = true));
    try {
      const { code, status: next } = await start();
      this.host.onStatus(next);
      this.screen("Sync is on", [el("p", { className: "np-path", textContent: next.folder ?? "", title: next.folder ?? "" }), ...this.codeBox(code)], [button("Done", () => this.close(), "primary")]);
      this.body.querySelector<HTMLElement>(".primary")?.focus();
    } catch (e) {
      if (!this.dialog.open) this.open(status);
      this.body.querySelectorAll<HTMLButtonElement>(".sd-app").forEach((b) => (b.disabled = false));
      this.showError(message(e));
    }
  }

  /** The code screen of "Open shared project…"; with `chooseFolder` (nothing found), a folder can be chosen. */
  private sharedScreen(code: string, error: string, chooseFolder: boolean): void {
    const input = this.codeInput(code);
    input.id = "sd-shared-code";
    input.placeholder = "Invite link or XXXX-XXXX-…";
    input.ariaLabel = "Invite link or project code";
    const form = this.codeForm(input, "Open", (value) => (isInviteLink(value) ? void this.joinServer(value) : void this.findShared(value, null)));
    const actions = [button("Cancel", () => this.close())];
    if (chooseFolder) actions.push(button("Choose folder…", () => void this.chooseSharedFolder(input)));
    const remembered = el("div", { className: "sd-remembered", hidden: true });
    this.screen(
      "Open shared project",
      [
        el("p", { textContent: "Paste the invite link you were sent, or the project code from the device that shares the project through a folder (Lethora looks for it in your sync apps)." }),
        el("label", { className: "sd-label", htmlFor: input.id, textContent: "Invite link or project code" }),
        form,
        remembered,
      ],
      actions,
    );
    this.showError(error);
    this.host.api.serverRemembered().then(
      (projects) => {
        if (!remembered.isConnected || projects.length === 0) return;
        const list = el("ul", { className: "sd-list" });
        for (const p of projects) {
          const open = button("Open", () => void this.reopen(p, open));
          open.title = `Download “${p.title}” from ${serverHost(p.server)} again`;
          list.append(el("li", { className: "sd-row" }, el("span", { className: "sd-row-main" }, el("span", { textContent: p.title }), el("span", { className: "sd-row-sub", textContent: serverHost(p.server) })), open));
        }
        remembered.replaceChildren(el("h3", { className: "sd-section", textContent: "From your servers" }), el("p", { className: "sd-note", textContent: "Projects you shared or joined before that have no copy on this device:" }), list);
        remembered.hidden = false;
      },
      (e) => this.showError(message(e)),
    );
  }

  /** Reopens remembered project `p` from its server. */
  private async reopen(p: RememberedProject, submit: HTMLButtonElement): Promise<void> {
    submit.disabled = true;
    try {
      this.screen("Opening…", [el("p", { textContent: `Downloading “${p.title}” from the server and decrypting it on this device.` })], []);
      await this.host.reopenServer(p.project);
      this.close();
      this.host.info("Opened. Changes from the others arrive live.");
    } catch (e) {
      if (!this.dialog.open) this.dialog.showModal();
      this.sharedScreen("", message(e), false);
    }
  }

  /** The server part of "Share / Sync": the address (remembered ones offered), the name if unknown, Share. */
  private serverForm(): Node[] {
    const url = el("input", { type: "url", id: "sd-server-url", className: "sd-code-input", placeholder: "https://lethora.example", spellcheck: false });
    const list = el("datalist", { id: "sd-servers" });
    url.setAttribute("list", list.id);
    const name = el("input", { type: "text", id: "sd-name", className: "sd-code-input sd-name", autocomplete: "name", spellcheck: false });
    const nameLabel = el("label", { className: "sd-label", htmlFor: name.id, textContent: "Your name (others in the project see it)", hidden: true });
    const nameRow = el("div", { className: "sd-code-form", hidden: true }, name);
    const share = el("button", { type: "button", className: "primary", textContent: "Share" });
    const form = el("form", { className: "sd-code-form" }, url, share);
    // Share sits on the last row shown: after the name when it is asked.
    const askName = (ask: boolean): void => {
      nameLabel.hidden = nameRow.hidden = !ask;
      (ask ? nameRow : form).append(share);
    };
    share.addEventListener("click", () => form.requestSubmit());
    name.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        form.requestSubmit();
      }
    });
    const warning = el("p", { className: "sd-note sd-warning", textContent: "Plain http: your text stays end-to-end encrypted, but the connection is not. Use it only for a server on your own network.", hidden: true });
    const progress = el("p", { className: "sd-note", hidden: true });
    url.addEventListener("input", () => (warning.hidden = !isPlainHttp(url.value)));
    let knownName: string | null = null;
    this.host.api.serverRecent().then(
      (recent) => {
        list.replaceChildren(...recent.servers.map((value) => el("option", { value })));
        if (!url.value && recent.servers[0]) url.value = recent.servers[0];
        warning.hidden = !isPlainHttp(url.value);
        knownName = recent.name;
        name.value = recent.name ?? "";
        askName(recent.name === null);
      },
      (e) => this.showError(message(e)),
    );
    form.addEventListener("submit", (e) => {
      e.preventDefault();
      const address = url.value.trim();
      const who = (name.value || knownName || "").trim();
      if (!address) return this.showError("Enter the server's address, like https://lethora.example.");
      if (!who) {
        askName(true);
        name.focus();
        return this.showError("Enter your name, as others will see it.");
      }
      void this.share(address, who, share, progress);
    });
    return [el("label", { className: "sd-label", htmlFor: url.id, textContent: "Server address" }), form, list, nameLabel, nameRow, warning, progress];
  }

  private async share(url: string, name: string, submit: HTMLButtonElement, progress: HTMLElement): Promise<void> {
    submit.disabled = true;
    this.showError("");
    progress.textContent = "Sharing… the project goes to the server, encrypted.";
    progress.hidden = false;
    try {
      const status = await this.host.api.serverShare(url, name);
      this.host.onStatus(status);
      this.render(status);
      this.host.info("Shared. Invite people from Share / Sync.");
    } catch (e) {
      progress.hidden = true;
      this.showError(message(e));
    } finally {
      submit.disabled = false;
    }
  }

  /** The dialog while the project is shared on a server. */
  private serverScreen(status: SyncStatus): void {
    const rows = el(
      "dl",
      { className: "sd-facts" },
      el("dt", { textContent: "Server" }),
      el("dd", { textContent: status.server ?? "", title: status.server ?? "" }),
      el("dt", { textContent: "Your role" }),
      el("dd", { textContent: status.role ? ROLE_NAMES[status.role] : "—" }),
      el("dt", { textContent: "Status" }),
      el("dd", { textContent: STATE_NAMES[status.state] ?? status.state }),
    );
    const problem = status.state !== "online" && status.state !== "connecting" && status.message ? [el("p", { textContent: status.message })] : [];
    const usable = status.state !== "removed" && status.state !== "needsCode" && status.state !== "needsUpdate";
    const actions: HTMLButtonElement[] = [];
    if (usable) actions.push(button("Members…", () => this.membersScreen(status)));
    if (usable && status.role === "owner") actions.push(button("Invite people…", () => this.inviteScreen(status)));
    if (usable) actions.push(button("Add my other device…", () => void this.deviceInvite(status)));
    actions.push(this.closeButton("primary"));
    const note = el("p", { className: "sd-note", textContent: "Folder sync is off while the project is shared on a server: one way at a time." });
    this.screen("Shared on a Lethora server", [rows, ...problem, note], actions);
  }

  /** "Members…": who is in the project and the open invites; the owner changes roles, removes members, revokes invites and deletes the project, anyone removes their own devices or leaves. */
  private membersScreen(status: SyncStatus): void {
    const back = button("Back", () => this.serverScreen(status));
    this.screen("Members", [el("p", { className: "sd-note", textContent: "Loading…" })], [back, this.closeButton("primary")]);
    this.host.api.serverMembers().then(
      (reply) => {
        if (this.dialog.open) this.renderMembers(status, reply);
      },
      (e) => this.showError(message(e)),
    );
  }

  private renderMembers(status: SyncStatus, reply: MembersReply): void {
    const owner = reply.members.find((m) => m.role === "owner");
    const isOwner = owner?.user === reply.user;
    const again = () => this.membersScreen(status);
    const list = el("ul", { className: "sd-list sd-members" });
    for (const m of reply.members.filter((m) => !m.removed)) list.append(this.memberRow(reply, m, isOwner, again));
    const content: Node[] = [list];
    // Removed members fold away below (their profiles still name their text).
    const removed = reply.members.filter((m) => m.removed);
    if (removed.length > 0) {
      const fold = el("ul", { className: "sd-list sd-members" });
      for (const m of removed) fold.append(this.memberRow(reply, m, isOwner, again));
      content.push(el("details", { className: "sd-fold" }, el("summary", { className: "sd-section", textContent: `Removed (${removed.length})` }), fold));
    }
    if (reply.invites.length > 0) {
      const invites = el("ul", { className: "sd-list" });
      for (const i of reply.invites) {
        const row = el("li", { className: "sd-row" }, el("span", { className: "sd-row-main", textContent: inviteSummary(i, reply.members, (ms) => new Date(ms).toLocaleDateString(undefined, { dateStyle: "medium" })) }));
        if (isOwner || i.creator === reply.device) {
          row.append(button("Revoke", () => this.confirmAction("Revoke this invite? Its link stops working at once.", "Revoke", () => this.host.api.serverRevoke(i.id), "Invite revoked.", again)));
        }
        invites.append(row);
      }
      content.push(el("h3", { className: "sd-section", textContent: "Open invites" }), invites);
    }
    const actions: HTMLButtonElement[] = [button("Back", () => this.serverScreen(status))];
    if (isOwner) actions.push(button("Delete from server…", () => this.deleteScreen(status, reply.title), "sd-danger"));
    else actions.push(button("Leave project…", () => this.confirmLeave(), "sd-danger"));
    actions.push(this.closeButton("primary"));
    this.screen("Members", content, actions);
  }

  /** One member: colour (on the own row a button that opens the swatches), name, role (a select for the owner), Remove; then its devices. */
  private memberRow(reply: MembersReply, m: MemberInfo, isOwner: boolean, again: () => void): HTMLLIElement {
    const you = m.user === reply.user && !m.removed;
    const dot = you ? el("button", { type: "button", className: "sd-dot sd-dot-pick", title: "Change your colour", ariaLabel: `Your colour: ${colourName(m.colour, reply.palette)}. Change it`, ariaExpanded: "false" }) : el("span", { className: "sd-dot" });
    dot.style.background = m.colour;
    const name = el("span", { className: "sd-row-main" }, dot, el("span", { textContent: `${memberName(m)}${you ? " (you)" : ""}` }));
    const row = el("li", { className: `sd-row${m.removed ? " sd-removed" : ""}` }, name);
    if (you) {
      const toggle = () => this.toggleSwatches(row, dot as HTMLButtonElement, m.colour, reply.palette, again);
      dot.addEventListener("click", toggle);
      // Escape closes open swatches (not the dialog).
      row.addEventListener("keydown", (e) => {
        if (e.key !== "Escape" || !row.querySelector(":scope > .sd-swatches")) return;
        e.preventDefault();
        e.stopPropagation();
        toggle();
      });
    }
    if (m.removed) {
      row.append(el("span", { className: "sd-row-sub", textContent: "Removed" }));
      return row;
    }
    if (isOwner && m.role !== "owner") {
      const role = select(`sd-role-${m.user}`, INVITE_ROLES.map((r) => [r.role, ROLE_NAMES[r.role]] as const), m.role);
      role.ariaLabel = `Role of ${memberName(m)}`;
      role.addEventListener("change", () => {
        role.disabled = true;
        this.host.api.serverSetRole(m.user, role.value as Exclude<Role, "owner">).then(
          () => {
            this.host.info(`${memberName(m)} is ${ROLE_NAMES[role.value as Role].toLowerCase()} now.`);
            again();
          },
          (e) => {
            role.value = m.role;
            role.disabled = false;
            this.showError(message(e));
          },
        );
      });
      row.append(role, button("Remove", () => this.confirmAction(`Remove ${memberName(m)} from the project with all their devices? They keep the copy they have, but get no more changes.`, "Remove", () => this.host.api.serverRemove({ user: m.user }), `${memberName(m)} was removed.`, again)));
    } else {
      row.append(el("span", { className: "sd-row-sub", textContent: ROLE_NAMES[m.role] }));
    }
    // Devices: the owner removes any but the owner's last; anyone their own (this one: leave).
    const devices = el("ul", { className: "sd-devices" });
    for (const d of m.devices) {
      const item = el("li", { className: "sd-row" }, el("span", { className: "sd-row-main sd-row-sub", textContent: deviceName(d, reply.device) }));
      const lastOfOwner = m.role === "owner" && m.devices.length === 1;
      if (d === reply.device && !lastOfOwner) item.append(button("Leave…", () => this.confirmLeave()));
      else if ((isOwner || you) && !lastOfOwner && d !== reply.device) {
        item.append(button("Remove", () => this.confirmAction(`Remove ${deviceName(d, reply.device).toLowerCase()} of ${memberName(m)}? It gets no more changes.`, "Remove", () => this.host.api.serverRemove({ device: d }), "Device removed.", again)));
      }
      devices.append(item);
    }
    row.append(devices);
    return row;
  }

  /** Opens (or closes) the colours under the own row; picking one changes this user's colour, Escape closes them. */
  private toggleSwatches(row: HTMLLIElement, dot: HTMLButtonElement, current: string, palette: readonly string[], again: () => void): void {
    const shown = row.querySelector(":scope > .sd-swatches");
    const close = () => {
      row.querySelector(":scope > .sd-swatches")?.remove();
      dot.ariaExpanded = "false";
      dot.focus();
    };
    if (shown) return close();
    const swatches = el("div", { className: "sd-swatches", role: "group", ariaLabel: "Your colour" });
    const enable = (on: boolean) => swatches.querySelectorAll("button").forEach((b) => (b.disabled = !on));
    for (const colour of palette) {
      const name = colourName(colour, palette);
      const pick = el("button", { type: "button", className: "sd-swatch", title: name, ariaLabel: name, ariaPressed: String(colour === current) });
      pick.style.background = colour;
      pick.addEventListener("click", () => {
        if (colour === current) return close();
        enable(false);
        this.host.api.serverSetColour(colour).then(
          () => {
            this.host.info(`Your colour is ${name.toLowerCase()} now.`);
            again();
          },
          (e) => {
            enable(true);
            this.showError(message(e));
          },
        );
      });
      swatches.append(pick);
    }
    row.insertBefore(swatches, row.querySelector(":scope > .sd-devices"));
    dot.ariaExpanded = "true";
    (swatches.querySelector<HTMLButtonElement>('[aria-pressed="true"]') ?? swatches.querySelector("button"))?.focus();
  }

  /** Asks `text` in place of the buttons; `label` runs `run`, then says `done` and goes on with `then`. */
  private confirmAction(text: string, label: string, run: () => Promise<unknown>, done: string, then: () => void): void {
    const actions = this.body.querySelector(".sd-actions");
    if (!actions) return;
    const cancel = button("Cancel", () => confirm.replaceWith(actions));
    const go = button(
      label,
      () => {
        go.disabled = true;
        run().then(
          () => {
            this.host.info(done);
            then();
          },
          (e) => {
            confirm.replaceWith(actions);
            this.showError(message(e));
          },
        );
      },
      "primary",
    );
    const confirm = el("div", { className: "lp-confirm sd-confirm" }, el("span", { textContent: text }), el("div", { className: "lp-confirm-actions" }, cancel, go));
    actions.replaceWith(confirm);
    cancel.focus();
  }

  /** "Leave project…": this device leaves; the copy here stays as a local project. */
  private confirmLeave(): void {
    this.confirmAction(
      "Leave the project on this device? The copy here stays as a local project and gets no more changes. To come back, ask for a new invite.",
      "Leave",
      async () => this.host.onStatus(await this.host.api.serverLeave()),
      "You left the project. This copy is a local project now.",
      () => this.close(),
    );
  }

  /** "Delete from server…": typing the project's title confirms it. */
  private deleteScreen(status: SyncStatus, title: string): void {
    const input = el("input", { type: "text", id: "sd-delete-title", className: "sd-code-input sd-name", autocomplete: "off", spellcheck: false });
    const remove = button(
      "Delete from server",
      () =>
        void (async () => {
          remove.disabled = true;
          try {
            this.host.onStatus(await this.host.api.serverDelete());
            this.close();
            this.host.info("Deleted from the server. This copy is a local project now.");
          } catch (e) {
            remove.disabled = input.value !== title;
            this.showError(message(e));
          }
        })(),
      "primary sd-danger",
    );
    remove.disabled = true;
    input.addEventListener("input", () => (remove.disabled = input.value !== title));
    this.screen(
      "Delete from server",
      [
        el("p", { textContent: "This deletes the project from the server for everyone: nobody gets changes any more, and invites stop working. Every copy on a device stays, this one as a local project." }),
        el("label", { className: "sd-label", htmlFor: input.id, textContent: `Type the project's title to confirm: ${title}` }),
        el("div", { className: "sd-code-form" }, input),
      ],
      [button("Back", () => this.membersScreen(status)), remove],
    );
    input.focus();
  }

  /** "Invite people…": role, expiry and uses, then the link. */
  private inviteScreen(status: SyncStatus): void {
    const role = select("sd-role", INVITE_ROLES.map((r) => [r.role, r.label] as const), "editor");
    const days = select("sd-days", INVITE_DAYS.map((d) => [String(d), d === 1 ? "1 day" : `${d} days`] as const), "7");
    const uses = select("sd-uses", INVITE_USES.map((u) => [String(u), u === 1 ? "1 person" : `${u} people`] as const), "1");
    const field = (label: string, control: HTMLSelectElement) => el("label", { className: "sd-field" }, el("span", { textContent: label }), control);
    const create = button(
      "Create link",
      () =>
        void this.invite(status, create, "Invite link", "Send this link privately. Whoever opens it with Open shared project… joins with that role, until it expires or is used up.", () =>
          this.host.api.serverInvite(role.value as Exclude<Role, "owner">, Number(days.value), Number(uses.value)),
        ),
      "primary",
    );
    this.screen(
      "Invite people",
      [el("p", { textContent: "Create a link for new members. Their role decides what they may change." }), el("div", { className: "sd-fields" }, field("Role", role), field("Expires after", days), field("Works for", uses))],
      [button("Back", () => this.serverScreen(status)), create],
    );
    role.focus();
  }

  /** "Add my other device…": a link for one more device of this user. */
  private async deviceInvite(status: SyncStatus): Promise<void> {
    const back = button("Back", () => this.serverScreen(status));
    this.screen("Add my other device", [el("p", { className: "sd-note", textContent: "Creating the link…" })], [back]);
    await this.invite(status, null, "Link for your other device", "On your other device, choose Open shared project… and paste this link. It works once, within 1 day.", () => this.host.api.serverInvite(null, 1, 1));
  }

  /** Creates an invite with `create` and shows its link. */
  private async invite(status: SyncStatus, submit: HTMLButtonElement | null, heading: string, note: string, create: () => Promise<string>): Promise<void> {
    if (submit) submit.disabled = true;
    try {
      const link = await create();
      const copy = button("Copy", () => {
        navigator.clipboard.writeText(link).then(
          () => (copy.textContent = "Copied"),
          () => this.showError("Could not copy. Select the link and copy it by hand."),
        );
      });
      this.screen(heading, [el("div", { className: "sd-code" }, el("code", { textContent: link }), copy), el("p", { className: "sd-note", textContent: note })], [button("Back", () => this.serverScreen(status)), button("Done", () => this.close(), "primary")]);
      copy.focus();
    } catch (e) {
      this.showError(message(e));
    } finally {
      if (submit) submit.disabled = false;
    }
  }

  /** Joins through invite link `link`; asks for the name first if it is unknown. */
  private async joinServer(link: string, name?: string): Promise<void> {
    const submit = this.body.querySelector<HTMLButtonElement>(".sd-code-form button");
    if (submit) submit.disabled = true;
    try {
      const who = name ?? (await this.host.api.serverRecent()).name;
      if (!who) {
        this.nameScreen(link);
        return;
      }
      this.screen("Joining…", [el("p", { textContent: "Downloading the project from the server and decrypting it on this device." })], []);
      await this.host.joinServer(link, who);
      this.close();
      this.host.info("Joined. Changes from the others arrive live.");
    } catch (e) {
      if (!this.dialog.open) this.dialog.showModal();
      this.sharedScreen(link, message(e), false);
    } finally {
      if (submit) submit.disabled = false;
    }
  }

  /** Asks for the user's name before joining through `link`. */
  private nameScreen(link: string): void {
    const input = el("input", { type: "text", id: "sd-join-name", className: "sd-code-input sd-name", autocomplete: "name", spellcheck: false });
    const form = el("form", { className: "sd-code-form" }, input, el("button", { type: "submit", className: "primary", textContent: "Join" }));
    form.addEventListener("submit", (e) => {
      e.preventDefault();
      const name = input.value.trim();
      if (name) void this.joinServer(link, name);
      else this.showError("Enter your name, as others will see it.");
    });
    this.screen("Your name", [el("p", { textContent: "Others in the project see this name next to your changes." }), el("label", { className: "sd-label", htmlFor: input.id, textContent: "Your name" }), form], [button("Cancel", () => this.close())]);
    input.focus();
  }

  /** Finds the project `code` unlocks (in `folder` if given) and joins it, or says why not. */
  private async findShared(code: string, folder: string | null): Promise<void> {
    const submit = this.body.querySelector<HTMLButtonElement>(".sd-code-form button");
    if (submit) submit.disabled = true;
    try {
      const found = await this.host.api.find(code, folder);
      if (found.local) {
        this.alreadyHere(found.local);
      } else if (!found.replica) {
        this.sharedScreen(code, `${notFoundMessage(found.searched, folder)} Choose the folder that holds it.`, true);
        this.body.querySelector<HTMLElement>(".sd-actions button:last-child")?.focus();
      } else {
        await this.host.join(found.replica, code);
        this.close();
        this.host.info("Opened. Changes from your other devices arrive automatically.");
      }
    } catch (e) {
      this.showError(message(e));
    } finally {
      if (submit) submit.disabled = false;
    }
  }

  /** "Choose folder…": the synced project's folder, or one holding it. */
  private async chooseSharedFolder(input: HTMLInputElement): Promise<void> {
    const code = input.value.trim();
    if (!code) {
      this.showError("Enter the project code.");
      return;
    }
    let folder: string | null;
    try {
      folder = await this.host.api.pickSharedFolder();
    } catch (e) {
      this.showError(message(e));
      return;
    }
    if (folder !== null) await this.findShared(code, folder);
  }

  /** The project is on this device already: open that one (a second copy would write as the same device). */
  private alreadyHere(path: string): void {
    const open = button(
      "Open it",
      () => {
        this.host.openProject(path).then(
          () => this.close(),
          (e) => this.showError(message(e)),
        );
      },
      "primary",
    );
    this.screen(
      "Already on this device",
      [
        el("p", { textContent: "This project is on this device already, syncing with the shared copy:" }),
        el("p", { className: "np-path", textContent: path, title: path }),
        el("p", { className: "sd-note", textContent: "A second copy here would write to the shared copy as the same device, so Lethora opens this one instead." }),
      ],
      [button("Cancel", () => this.close()), open],
    );
    open.focus();
  }

  private async revealCode(slot: HTMLElement, show: HTMLButtonElement): Promise<void> {
    try {
      slot.replaceChildren(...this.codeBox(await this.host.api.code()));
      show.hidden = true;
    } catch (e) {
      this.showError(message(e));
    }
  }

  private async enterCode(code: string): Promise<void> {
    try {
      const status = await this.host.api.enterCode(code);
      this.host.onStatus(status);
      this.render(status);
      this.host.info("Connected to the sync folder.");
    } catch (e) {
      this.showError(message(e));
    }
  }

  /** Asks before stopping (in place of the buttons). */
  private confirmStop(): void {
    const actions = this.body.querySelector(".sd-actions")!;
    const cancel = button("Cancel", () => confirm.replaceWith(actions));
    const stop = button("Stop syncing", () => void this.stop(), "primary");
    const confirm = el(
      "div",
      { className: "lp-confirm sd-confirm" },
      el("span", { textContent: "Stop syncing on this device? The encrypted copy stays in the sync folder, and your other devices keep syncing with it." }),
      el("div", { className: "lp-confirm-actions" }, cancel, stop),
    );
    actions.replaceWith(confirm);
    cancel.focus();
  }

  private async stop(): Promise<void> {
    try {
      const status = await this.host.api.stop();
      this.host.onStatus(status);
      this.render(status);
      this.host.info("Sync stopped on this device.");
    } catch (e) {
      this.showError(message(e));
    }
  }
}

/** The one-time notice for a project in a sync client's folder (synced in plain text). */
export class SyncNotice {
  private el = byId("sync-notice");
  private text = byId("sync-notice-text");

  constructor(handlers: { setup(): void; dismiss(): Promise<void>; error(e: unknown): void }) {
    byId("sync-notice-setup").addEventListener("click", () => {
      this.hide();
      handlers.setup();
    });
    byId("sync-notice-dismiss").addEventListener("click", () => {
      this.hide();
      handlers.dismiss().catch(handlers.error);
    });
    byId("sync-notice-close").addEventListener("click", () => this.hide());
  }

  show(provider: string): void {
    this.text.textContent = `This project is in a ${provider} folder, so it syncs in plain text. Encrypted sync keeps it private.`;
    this.el.hidden = false;
  }

  hide(): void {
    this.el.hidden = true;
  }
}
