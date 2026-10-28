// The manual's screenshots, taken by screenshots.mjs (see there). Each shot:
//   name      file name: docs/images/<name>.png
//   viewport  [width, height] in CSS px (default [1400, 850])
//   scale     device pixel ratio (default 1; 2 for crisp images)
//   scheme    "light" (default) or "dark"
//   query     appended to the URL, e.g. "?book=1"
//   storage   localStorage entries set before the app loads (all else cleared),
//             e.g. "lethora.view": "science" | "publisher", "lethora.focus": "1"
//   steps     run in order: a string is JS run in the page (may await); objects:
//             { click: selector, text?, x?, y? }  real mouse click (first match containing text)
//             { hover: selector, text? }          moves the mouse there
//             { key: "F6", ctrl?, shift?, alt? }  keydown + keyup at the focused element
//             { wait: selector, text?, timeout? } until it exists
//             { sleep: ms }
//   settle    ms to wait after the steps (default 600)
//   callouts  [{ selector, n, text?, at?, dx?, dy?, outline? }]: numbered circles at the
//             element's "top-right" (default), "top", "bottom", "left", "right" or
//             "center", moved by dx/dy px; outline draws a box around the element
//   clip      selector or { selector, padding }: capture only that element's box

/** Puts the cursor into the paragraph starting with `text`. */
const into = (text) => ({ click: ".cm-line", text, x: 40, y: 8 });

/** Scrolls the page preview so that page `n` is at the top. */
const toPage = (n) => `
  const s = document.getElementById("pub-scroll");
  const pause = () => new Promise((r) => setTimeout(r, 150));
  const find = () => [...s.querySelectorAll(".pub-page")].find((p) => p.querySelector(".pub-label").textContent === "${n}");
  // Pages exist only near the viewport.
  for (let i = 0; i < 50 && !find(); i++) {
    s.scrollTop += s.clientHeight / 2;
    await pause();
  }
  s.scrollTop += find().getBoundingClientRect().top - s.getBoundingClientRect().top - 12;
  await pause();`;

export const shots = [
  {
    name: "author-focus",
    storage: { "lethora.focus": "1" },
    steps: [into("Her father kept a logbook"), { sleep: 2500 }],
  },
  {
    name: "science-palette",
    storage: { "lethora.view": "science" },
    steps: [into("The lens turned once"), { click: "#toggle-palette" }, { wait: "#palette:not([hidden]) .palette-grid" }, { sleep: 2500 }],
  },
  {
    name: "publisher",
    storage: { "lethora.view": "publisher" },
    steps: [{ wait: ".pub-page.loaded img" }, { click: "#zoom-out" }, { click: "#zoom-out" }, { click: "#zoom-out" }, { click: "#zoom-out" }, toPage(3), { wait: ".pub-page.loaded .pub-mark" }, { hover: "#export-pdf" }],
    // 1 zoom, 2 the View menu (markers, Exceptions drawer, zoom, Compile), 3 Export PDF.
    callouts: [
      { selector: ".bar-end .pub-zoom", n: 1, at: "bottom", dy: 18, outline: true },
      { selector: ".mb-top", text: "View", n: 2, at: "bottom", dy: 18, outline: true },
      { selector: "#export-pdf", n: 3, at: "bottom", dy: 18, outline: true },
    ],
  },
  {
    name: "split-exceptions",
    steps: [into("The keepers before them"), { key: "F6" }, { wait: "#publisher:not([hidden]) .pub-page.loaded img" }, toPage(3), { key: "F7" }, { sleep: 2500 }],
  },
  {
    name: "assets-panel",
    storage: { "lethora.view": "science", "lethora.assetsFolder": "assets/photos/" },
    steps: [into("The lens turned once"), { click: "#toggle-assets" }, { wait: "#assets .as-thumb img" }, { sleep: 1500 }],
    callouts: [
      { selector: "#assets .as-tree", n: 1, at: "top", dy: 14 },
      { selector: "#assets .as-search", n: 2, at: "top", dy: -14 },
      { selector: "#assets .as-cloud", n: 3, at: "right", dx: 18 },
    ],
  },
  {
    name: "assets-drop",
    storage: { "lethora.assetsMode": "list" },
    steps: [
      { click: "#toggle-assets" },
      { wait: "#assets .as-row" },
      // A panel item dragged over the text: the marker shows where the figure goes.
      `const line = [...document.querySelectorAll("#editor .cm-line")].find((l) => l.textContent.startsWith("Her father kept"));
      const r = line.getBoundingClientRect();
      const dt = new DataTransfer();
      dt.setData("application/x-lethora-asset", "assets/lighthouse.png");
      document.querySelector("#editor .cm-scroller").dispatchEvent(new DragEvent("dragover", { bubbles: true, cancelable: true, dataTransfer: dt, clientX: r.left + 40, clientY: r.top + 4 }));`,
      { wait: ".cm-drop-marker" },
    ],
    callouts: [{ selector: ".cm-drop-marker", n: 1, at: "left", dx: -14 }],
  },
  {
    // One-click setup in Dropbox (dev mock): the project code to paste on the other device (random in the mock).
    name: "share-sync",
    steps: [{ click: "#sync-cloud" }, { click: "#sd-body .sd-app", text: "Dropbox" }, { wait: "#sd-body .sd-code code" }],
    callouts: [{ selector: "#sync-cloud", n: 1, at: "bottom", dy: 16, outline: true }],
  },
  {
    // "Open shared project…" on the other device, with a pasted code.
    name: "open-shared",
    steps: [
      { click: "#sync-cloud" },
      { click: "#sd-body button", text: "Open shared project" },
      { wait: "#sd-shared-code" },
      `document.getElementById("sd-shared-code").value = "7KQ2-M9XD-4TPA-HV3E-R8ZN-6WFC-2BJY-Q5SG-0NKT-D7MV-X3EH-9RAP-4YCW-1ZFB";`,
    ],
  },
  {
    // Shared on a server (dev mock): Members… with the own colour swatches and the removed members unfolded.
    name: "members",
    query: "?sync=server",
    steps: [
      { click: "#sync-cloud" },
      { click: "#sd-body button", text: "Members" },
      { wait: "#sd-body .sd-dot-pick" },
      { click: "#sd-body .sd-fold summary" },
      { click: "#sd-body .sd-dot-pick" },
      { wait: "#sd-body .sd-swatches" },
    ],
  },
  {
    // Invite people… → Create link: the link to send (made up in the mock).
    name: "invite-link",
    query: "?sync=server",
    steps: [
      { click: "#sync-cloud" },
      { click: "#sd-body button", text: "Invite people" },
      { wait: "#sd-role" },
      { click: "#sd-body button", text: "Create link" },
      { wait: "#sd-body .sd-code code" },
    ],
  },
  {
    // Bea's caret with its name flag (shown briefly after each move), who's online in the title bar.
    name: "presence",
    query: "?presence=1",
    steps: [
      { wait: ".cm-remote-caret" },
      // Right after the next move, while the flag shows.
      `const first = document.querySelector(".cm-remote-caret");
      while (document.querySelector(".cm-remote-caret") === first) await new Promise((r) => setTimeout(r, 50));`,
    ],
    settle: 300,
    callouts: [
      { selector: "#online", n: 1, at: "bottom", dy: 16, outline: true },
      { selector: ".cm-remote-flag", n: 2, at: "right", dx: 18 },
    ],
  },
  {
    // View → Show authors: each author's text in their colour, the legend bottom right.
    name: "show-authors",
    query: "?presence=1",
    steps: [
      { click: ".mb-top", text: "View" },
      { click: ".mb-item[data-command='authors']" },
      { wait: "#authors-legend:not([hidden])" },
      { sleep: 800 },
    ],
    callouts: [{ selector: "#authors-legend", n: 1, at: "left", dx: -18 }],
  },
  {
    // View → Review as an editor: cards beside their lines, one thread open, highlights in the text.
    name: "review-panel",
    query: "?sync=server-editor",
    storage: { "lethora.review": "1" },
    // (The cards are placed after they are listed: the click waits for them.)
    steps: [{ wait: "#review .rv-card" }, { sleep: 1000 }, { click: "#review .rv-card .rv-text", text: "Lovely opening" }, { wait: "#review .rv-card.active .rv-reply" }, { sleep: 800 }],
    // 1 the thread's card, 2 a highlight, 3 the filter.
    callouts: [
      { selector: "#review .rv-card.active", n: 1, at: "left", dx: -16 },
      { selector: ".cm-review-yellow", n: 2, at: "top", dy: -14 },
      { selector: "#review .rv-filter", n: 3, at: "bottom", dy: 16, outline: true },
    ],
  },
  {
    // A reviewer types over "logbook": a suggestion of their own beside Bea's (struck and underlined text).
    name: "suggestions",
    query: "?sync=server-reviewer",
    storage: { "lethora.review": "1" },
    steps: [
      { wait: "#review .rv-card" },
      into("Her father kept a logbook"),
      `const line = [...document.querySelectorAll("#editor .cm-line")].find((l) => l.textContent.startsWith("Her father kept"));
      const walk = document.createTreeWalker(line, NodeFilter.SHOW_TEXT);
      let node;
      while ((node = walk.nextNode()) && !node.data.includes("logbook"));
      const at = node.data.indexOf("logbook");
      document.getSelection().setBaseAndExtent(node, at, node, at + "logbook".length);
      await new Promise((r) => setTimeout(r, 200));
      document.execCommand("insertText", false, "journal");`,
      { wait: ".cm-suggest-ins", text: "journal" },
      { wait: "#review .rv-card.active button", text: "Accept" },
      { sleep: 800 },
    ],
    // 1 the role badge, 2 the typed suggestion, 3 Accept, disabled for a reviewer.
    callouts: [
      { selector: "#read-only", n: 1, at: "bottom", dy: 16, outline: true },
      { selector: ".cm-suggest-ins", text: "journal", n: 2, at: "top", dy: -14 },
      { selector: "#review .rv-card.active button", text: "Accept", n: 3, at: "left", dx: -16 },
    ],
  },
  {
    // Right-click on Bea's suggestion as an editor: Accept and Reject.
    name: "review-menu",
    query: "?sync=server-editor",
    storage: { "lethora.review": "1" },
    steps: [
      { wait: ".cm-suggest-ins", text: "reached" },
      `document.querySelector("#editor .cm-scroller").scrollTop += 340;
      await new Promise((r) => setTimeout(r, 500));
      // At the end of the inserted text: the menu leaves both parts of the suggestion visible.
      const el = [...document.querySelectorAll(".cm-suggest-ins")].find((e) => e.textContent === "reached");
      const r = el.getBoundingClientRect();
      el.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, button: 2, clientX: r.right - 2, clientY: r.top + r.height / 2 }));`,
      { wait: ".page-menu .pm-item", text: "Accept suggestion" },
      { hover: ".page-menu .pm-item", text: "Accept suggestion" },
    ],
    callouts: [{ selector: ".page-menu .pm-item", text: "Accept suggestion", n: 1, at: "right", dx: 18 }],
  },
  {
    // File → Open recent with the dev mock's seeded projects (one gone from disk).
    name: "menus",
    viewport: [1100, 620],
    steps: [
      { click: ".mb-top", text: "File" },
      { wait: ".mb-drop:not([hidden]) .mb-item" },
      { hover: ".mb-item[data-command='open-recent']" },
      { wait: ".mb-sub .mb-item" },
      { hover: ".mb-sub .mb-item" },
    ],
  },
  {
    // The start screen after File → Close project (Ctrl+W): the closed project heads the Recent list.
    name: "start-screen",
    steps: [{ key: "w", ctrl: true }, { wait: "#recent:not([hidden]) .recent-open" }],
  },
];
