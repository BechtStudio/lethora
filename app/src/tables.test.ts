// Tests for the inserted table markup, its label and where it goes. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { ensureSyntaxTree } from "@codemirror/language";
import { EditorSelection, EditorState, Text, type TransactionSpec } from "@codemirror/state";
import { typstLanguage } from "./typst.ts";
import { insertTable, nextCell, nextTableLabel, previousCell, sizeLabel, type TableAction, tableAt, tableCommand, tableMenu, tablePlace, tableText } from "./tables.ts";

test("table text: header row counted in the rows, empty caption, cursor in the first cell", () => {
  const { text, cell } = tableText(3, 3, true, "tab:1");
  assert.equal(
    text,
    `#figure(
  table(
    columns: 3,
    table.header([], [], []),
    [], [], [],
    [], [], [],
  ),
  caption: [],
) <tab:1>`,
  );
  assert.equal(text.slice(cell - 1, cell + 1), "[]");
  assert.ok(text.slice(0, cell).endsWith("table.header(["));
  // Without a header: every row a body row.
  const plain = tableText(2, 2, false, "tab:7").text;
  assert.equal(plain.match(/^ {4}\[\], \[\],$/gm)?.length, 2);
  assert.ok(!plain.includes("table.header"));
  assert.ok(tableText(2, 2, false, "x").text.slice(0, tableText(2, 2, false, "x").cell).endsWith("columns: 2,\n    ["));
  // One row with a header: the header only. Out of range: clamped.
  assert.ok(tableText(1, 1, true, "t").text.includes("    table.header([]),\n  ),"));
  assert.ok(tableText(99, 0, false, "t").text.includes("columns: 10,"));
  assert.equal(sizeLabel(3, 4), "3 × 4 table");
});

test("next free tab:N label", () => {
  assert.equal(nextTableLabel([]), "tab:1");
  assert.equal(nextTableLabel(["fig:a", "tab:2", "tab:10", "tab:x", "tab:3b", "eq:1"]), "tab:11");
});

/** The document after inserting, with `|` at the cursor. */
function insert(doc: string, at: number, header = true): string {
  const state = EditorState.create({ doc, selection: EditorSelection.cursor(at) });
  const s = state.update(insertTable(state, 1, 2, header, "tab:1")).state;
  const text = s.doc.toString();
  const head = s.selection.main.head;
  return text.slice(0, head) + "|" + text.slice(head);
}

const TABLE = "#figure(\n  table(\n    columns: 1,\n    table.header([|]),\n    [],\n  ),\n  caption: [],\n) <tab:1>";

test("inserted as a block of its own", () => {
  // On an empty line: there, blank lines towards text.
  assert.equal(insert("Para.\n\nNext.", 6), `Para.\n\n${TABLE}\n\nNext.`);
  assert.equal(insert("Para.\n\n\nNext.", 6), `Para.\n\n${TABLE}\n\nNext.`);
  assert.equal(insert("", 0), `${TABLE}\n`);
  assert.equal(insert("Para.\n", 6), `Para.\n\n${TABLE}\n`);
  // In a paragraph: after it, never inside.
  assert.equal(insert("One line\ntwo line\n\nNext.", 3), `One line\ntwo line\n\n${TABLE}\n\nNext.`);
  assert.equal(insert("Last words", 4), `Last words\n\n${TABLE}\n`);
  assert.equal(insert("A\nB", 0), `A\nB\n\n${TABLE}\n`);
});

test("place of a table", () => {
  const doc = Text.of(["Para.", "  ", "Next."]);
  assert.deepEqual(tablePlace(doc, 6), { from: 6, to: 8, before: "\n", after: "\n" });
  assert.deepEqual(tablePlace(doc, 2), { from: 5, to: 5, before: "\n\n", after: "" });
});

// ---- Editing a table on the code ----

/** A state from `doc` with `|` at the cursor, or `«` (anchor) and `»` (head) around a selection; Typst parsed. */
function stateAt(marked: string, readOnly = false): EditorState {
  let anchor = 0;
  let head = 0;
  [...marked.matchAll(/[|«»]/g)].forEach((m, i) => {
    const pos = m.index! - i;
    if (m[0] === "|") anchor = head = pos;
    else if (m[0] === "«") anchor = pos;
    else head = pos;
  });
  const doc = marked.replace(/[|«»]/g, "");
  const state = EditorState.create({ doc, selection: EditorSelection.single(anchor, head), extensions: [typstLanguage, EditorState.readOnly.of(readOnly)] });
  ensureSyntaxTree(state, state.doc.length, 5000);
  return state;
}

/** The document with the selection marked (`|` a cursor, `«…»` a selection). */
function marked(state: EditorState): string {
  const { from, to } = state.selection.main;
  const text = state.doc.toString();
  if (from === to) return text.slice(0, from) + "|" + text.slice(from);
  return text.slice(0, from) + "«" + text.slice(from, to) + "»" + text.slice(to);
}

/** Runs `command` on `doc`: the result marked, null if it doesn't apply. */
function run(doc: string, command: (s: EditorState) => TransactionSpec | null): string | null {
  const state = stateAt(doc);
  const spec = command(state);
  return spec && marked(state.update(spec).state);
}

const act = (action: TableAction) => (s: EditorState) => tableCommand(s, action);

const SAMPLE = `Text.

#figure(
  table(
    columns: 3,
    table.header([Night], [Ships], [Weather]),
    [Monday], [4], [Fog],
    [Tuesday], [7], [Clear],
  ),
  caption: [Logbook.],
) <tab:1>

After.`;

/** SAMPLE with a cursor at the first `needle` (plus `shift`). */
const sampleAt = (needle: string, shift = 0) => {
  const i = SAMPLE.indexOf(needle) + shift;
  return SAMPLE.slice(0, i) + "|" + SAMPLE.slice(i);
};

test("table at the cursor: columns, header, cells as written, placement", () => {
  const t = tableAt(stateAt(sampleAt("[4]", 1)))!;
  assert.ok(t);
  assert.equal(t.fixed, null);
  assert.equal(t.cols, 3);
  assert.equal(t.rows, 3);
  assert.equal(t.headRows, 1);
  assert.deepEqual(t.header, []);
  assert.deepEqual(
    t.named.map((n) => n.src),
    ["columns: 3"],
  );
  assert.deepEqual(
    t.cells.map((c) => [c.src, c.row, c.col]),
    [
      ["[Night]", 0, 0],
      ["[Ships]", 0, 1],
      ["[Weather]", 0, 2],
      ["[Monday]", 1, 0],
      ["[4]", 1, 1],
      ["[Fog]", 1, 2],
      ["[Tuesday]", 2, 0],
      ["[7]", 2, 1],
      ["[Clear]", 2, 2],
    ],
  );
  assert.equal(t.indent, "  ");
  assert.equal(t.inner, "    ");
  // Outside the parentheses (the caption, `table` itself, after `)`), plain text: no table.
  assert.equal(tableAt(stateAt(sampleAt("Logbook"))), null);
  assert.equal(tableAt(stateAt(sampleAt("table(", 2))), null);
  assert.equal(tableAt(stateAt(sampleAt("  ),", 3))), null);
  assert.equal(tableAt(stateAt("Text |here.")), null);
  // Just after `(` and just before `)`: in it.
  assert.ok(tableAt(stateAt(sampleAt("table(", 6))));
  assert.ok(tableAt(stateAt(sampleAt("  ),", 2))));
});

test("table: track lists, no columns, named arguments kept, commas and brackets inside cells", () => {
  const doc = `#table(columns: (1fr, auto, 2cm), align: center, stroke: (x, y) => if y == 0 { 1pt },
  [a, b], ["x, y"], [#f(1, [z])],
  str(1), [\\]], [/* , */ c], [\`raw, ]\`], [$a, b$], [// no
  ], [tail|],
)`;
  const t = tableAt(stateAt(doc))!;
  assert.ok(t);
  assert.equal(t.fixed, null);
  assert.deepEqual(t.columns, { kind: "tracks", tracks: ["1fr", "auto", "2cm"] });
  assert.deepEqual(
    t.named.map((n) => n.name),
    ["columns", "align", "stroke"],
  );
  assert.equal(t.named[2].src, "stroke: (x, y) => if y == 0 { 1pt }");
  assert.deepEqual(
    t.cells.map((c) => c.src),
    ["[a, b]", '["x, y"]', "[#f(1, [z])]", "str(1)", "[\\]]", "[/* , */ c]", "[`raw, ]`]", "[$a, b$]", "[// no\n  ]", "[tail]"],
  );
  assert.equal(t.rows, 4);
  assert.equal(t.indent, "");
  assert.equal(t.inner, "  ");
  const one = tableAt(stateAt("#table([a], [b|])"))!;
  assert.deepEqual([one.cols, one.rows, one.columns.kind], [1, 2, "none"]);
  assert.deepEqual(tableAt(stateAt("#table(columns: auto, [a|])"))!.columns, { kind: "tracks", tracks: ["auto"] });
  assert.equal(tableAt(stateAt("#table(columns: (auto,), [a|])"))!.cols, 1);
});

test("table: merged cells placed like Typst (rowspans skip slots, colspans take several)", () => {
  const doc = `#table(
  columns: 3,
  table.cell(rowspan: 2, fill: red)[A], table.cell(colspan: 2)[B],
  [C], [D],
  [E], [F], [G|],
)`;
  const t = tableAt(stateAt(doc))!;
  assert.equal(t.fixed, null);
  assert.deepEqual(
    t.cells.map((c) => [c.row, c.col, c.rowspan, c.colspan]),
    [
      [0, 0, 2, 1],
      [0, 1, 1, 2],
      [1, 1, 1, 1],
      [1, 2, 1, 1],
      [2, 0, 1, 1],
      [2, 1, 1, 1],
      [2, 2, 1, 1],
    ],
  );
  assert.deepEqual(t.cells[0].call, { args: ["fill: red"], body: "[A]", trailing: true });
  // A body given as an argument.
  const arg = tableAt(stateAt("#table(columns: 2, table.cell(colspan: 2, [wide|]))"))!;
  assert.equal(arg.cells[0].colspan, 2);
  assert.deepEqual(arg.cells[0].call, { args: [], body: "[wide]", trailing: false });
});

test("table: multi-row header, the header's named arguments", () => {
  const t = tableAt(stateAt("#table(columns: 2, table.header(repeat: false, table.cell(colspan: 2)[T], [a], [b]), [c], [d|])"))!;
  assert.equal(t.headRows, 2);
  assert.equal(t.rows, 3);
  assert.deepEqual(t.header, ["repeat: false"]);
});

test("not editable: trailing content, spreads, unclosed, not code; fixed: lines, footer, x/y, comments, odd columns", () => {
  assert.equal(tableAt(stateAt("#table(columns: 2)[a|][b]")), null);
  assert.equal(tableAt(stateAt("#table(columns: 2, ..cells, [a|])")), null);
  assert.equal(tableAt(stateAt("#table(columns: 2, [a|]")), null);
  assert.equal(tableAt(stateAt("// table([a|])")), null);
  assert.equal(tableAt(stateAt("A mytable([a|]) here.")), null);
  const fixed = (doc: string) => tableAt(stateAt(doc))!.fixed ?? "";
  assert.match(fixed("#table(columns: 2, table.hline(), [a|], [b])"), /lines/);
  assert.match(fixed("#table(columns: 2, [a|], [b], table.footer([c], [d]))"), /footer/);
  assert.match(fixed("#table(columns: 2, table.cell(x: 1)[a|], [b])"), /x or y/);
  assert.match(fixed("#table(columns: 2, // note\n [a|], [b])"), /comments/);
  assert.match(fixed("#table(columns: (auto,) * 2, [a|], [b])"), /columns/);
  assert.match(fixed("#table(columns: 2, [a|], table.header([h]))"), /header/);
  assert.match(fixed("#table(columns: 2, table.cell(colspan: 3)[a|])"), /merged/);
  assert.match(fixed("#table(columns: 2, table.cell(colspan: n)[a|])"), /merged/);
  // Fixed: Tab still moves; rows, columns and the header stay; the table can still be deleted.
  const lines = "#table(columns: 2, table.hline(), [a|], [b])";
  assert.equal(run(lines, nextCell), "#table(columns: 2, table.hline(), [a], [«b»])");
  assert.equal(run(lines, act("row-below")), null);
  assert.equal(run(lines, act("header")), null);
  assert.equal(run(lines, act("delete-table")), "|");
  // Tab in the last cell of a fixed table stays.
  assert.equal(run("#table(columns: 2, table.hline(), [a], [b|])", nextCell), "#table(columns: 2, table.hline(), [a], [b|])");
});

test("Tab and Shift+Tab: the next / previous cell's content selected, header cells included", () => {
  assert.equal(run(sampleAt("[Night]", 1), nextCell), SAMPLE.replace("[Ships]", "[«Ships»]"));
  assert.equal(run(sampleAt("[Fog]", 2), nextCell), SAMPLE.replace("[Tuesday]", "[«Tuesday»]"));
  assert.equal(run(sampleAt("[Monday]", 3), previousCell), SAMPLE.replace("[Weather]", "[«Weather»]"));
  // The first cell: Shift+Tab stays (a no-op, not null).
  assert.deepEqual(previousCell(stateAt(sampleAt("[Night]", 1))), {});
  // Outside the cells (the columns line): the cell after; none before.
  assert.equal(run(sampleAt("columns"), nextCell), SAMPLE.replace("[Night]", "[«Night»]"));
  assert.equal(run(sampleAt("columns"), previousCell), sampleAt("columns"));
  // An empty cell: the cursor inside; an expression: all of it; a merged cell: its content.
  assert.equal(run("#table(columns: 2, [a|], [], str(1))", nextCell), "#table(columns: 2, [a], [|], str(1))");
  assert.equal(run("#table(columns: 2, [a], [|], str(1))", nextCell), "#table(columns: 2, [a], [], «str(1)»)");
  assert.equal(run("#table(columns: 2, [a|], table.cell(fill: red)[b])", nextCell), "#table(columns: 2, [a], table.cell(fill: red)[«b»])");
  // Outside a table and read only: not handled (Tab does what it does elsewhere).
  assert.equal(nextCell(stateAt("Plain |text.")), null);
  assert.equal(previousCell(stateAt("Plain |text.")), null);
  assert.equal(nextCell(stateAt(sampleAt("[Night]", 1), true)), null);
  assert.equal(tableCommand(stateAt(sampleAt("[Night]", 1), true), "row-below"), null);
});

test("Tab in the last cell: a new empty row, the cursor in its first cell", () => {
  assert.equal(run(sampleAt("[Clear]", 3), nextCell), SAMPLE.replace("[Tuesday], [7], [Clear],", "[Tuesday], [7], [Clear],\n    [|], [], [],"));
  // A one-line table is printed one row per line.
  assert.equal(run("#table(columns: 2, [a], [b|])", nextCell), "#table(\n  columns: 2,\n  [a], [b],\n  [|], [],\n)");
});

/** The changes of a spec, as [from, to, insert]. */
function changesOf(state: EditorState, spec: TransactionSpec): Array<[number, number, string]> {
  const out: Array<[number, number, string]> = [];
  state.update(spec).changes.iterChanges((from, to, _a, _b, text) => out.push([from, to, text.toString()]));
  return out;
}

test("the edit is the span that changed, never `table(` itself", () => {
  const state = stateAt(sampleAt("[Clear]", 3));
  const changes = changesOf(state, nextCell(state)!);
  assert.equal(changes.length, 1);
  const [from, to, insert] = changes[0];
  assert.equal(from, to);
  assert.equal(from, SAMPLE.indexOf("  ),") + 2);
  assert.equal(insert, "  [], [], [],\n  ");
  // A column: from the column count on.
  const col = stateAt(sampleAt("[Ships]", 1));
  const [[cf]] = changesOf(col, tableCommand(col, "column-right")!);
  assert.equal(cf, SAMPLE.indexOf("3,"));
  // Never before `table(` (an exception anchored there stays).
  const del = stateAt(sampleAt("[Ships]", 1));
  for (const action of ["row-above", "header", "delete-row", "merge"] as const) {
    const spec = tableCommand(del, action);
    if (spec) for (const [f] of changesOf(del, spec)) assert.ok(f >= SAMPLE.indexOf("table(") + "table(".length, action);
  }
});

test("insert rows and columns: empty cells, header kept, merged cells across grow", () => {
  assert.equal(run(sampleAt("[4]", 1), act("row-above")), SAMPLE.replace("    [Monday]", "    [|], [], [],\n    [Monday]"));
  assert.equal(run(sampleAt("[4]", 1), act("row-below")), SAMPLE.replace("[Fog],", "[Fog],\n    [|], [], [],"));
  // Below the header: a body row; above it: a header row.
  assert.equal(run(sampleAt("[Ships]", 1), act("row-below")), SAMPLE.replace("[Weather]),", "[Weather]),\n    [|], [], [],"));
  assert.equal(
    run(sampleAt("[Ships]", 1), act("row-above")),
    SAMPLE.replace("    table.header([Night], [Ships], [Weather]),", "    table.header(\n      [|], [], [],\n      [Night], [Ships], [Weather],\n    ),"),
  );
  assert.equal(
    run(sampleAt("[4]", 1), act("column-left")),
    SAMPLE.replace("columns: 3", "columns: 4").replace("[Night], [Ships]", "[Night], [], [Ships]").replace("[Monday], [4]", "[Monday], [|], [4]").replace("[Tuesday], [7]", "[Tuesday], [], [7]"),
  );
  assert.equal(
    run(sampleAt("[Fog]", 1), act("column-right")),
    SAMPLE.replace("columns: 3", "columns: 4").replace("[Weather]", "[Weather], []").replace("[Fog]", "[Fog], [|]").replace("[Clear]", "[Clear], []"),
  );
  // A track list gets an `auto` track; without columns, they are added.
  assert.equal(run("#table(columns: (1fr, 2cm), [a|], [b])", act("column-right")), "#table(\n  columns: (1fr, auto, 2cm),\n  [a], [|], [b],\n)");
  assert.equal(run("#table([a|])", act("column-left")), "#table(\n  columns: 2,\n  [|], [a],\n)");
  // Above a merged cell: a full row; across one: it grows.
  const merged = "#table(\n  columns: 2,\n  table.cell(rowspan: 2)[A], [b],\n  [c],\n)";
  assert.equal(run(merged.replace("[A]", "[A|]"), act("row-above")), "#table(\n  columns: 2,\n  [|], [],\n  table.cell(rowspan: 2)[A], [b],\n  [c],\n)");
  assert.equal(run(merged.replace("[c]", "[c|]"), act("row-above")), "#table(\n  columns: 2,\n  table.cell(rowspan: 3)[A], [b],\n  [|],\n  [c],\n)");
  assert.equal(run("#table(\n  columns: 2,\n  table.cell(colspan: 2)[A],\n  [b|], [c],\n)", act("column-right")), "#table(\n  columns: 3,\n  table.cell(colspan: 3)[A],\n  [b], [|], [c],\n)");
});

test("delete rows and columns: merged cells shrink, cells only there go; the last row or column deletes the table", () => {
  assert.equal(run(sampleAt("[4]", 1), act("delete-row")), SAMPLE.replace("    [Monday], [4], [Fog],\n", "").replace("[7]", "[7|]"));
  assert.equal(run(sampleAt("[4]", 1), act("delete-column")), SAMPLE.replace("columns: 3", "columns: 2").replace("[Ships], ", "").replace("[4], [Fog]", "[Fog|]").replace("[7], ", ""));
  // The header's only row: the header goes.
  assert.equal(run(sampleAt("[Ships]", 1), act("delete-row")), SAMPLE.replace("    table.header([Night], [Ships], [Weather]),\n", "").replace("[4]", "[4|]"));
  // The selected rows go together.
  assert.equal(
    run(SAMPLE.replace("[Monday]", "[«Monday]").replace("[7]", "[7»]"), act("delete-row")),
    SAMPLE.replace("    [Monday], [4], [Fog],\n    [Tuesday], [7], [Clear],\n", "").replace("[Ships]", "[Ships|]"),
  );
  // Merged cells shrink.
  assert.equal(run("#table(\n  columns: 2,\n  table.cell(rowspan: 2)[A], [b|],\n  [c],\n  [d], [e],\n)", act("delete-row")), "#table(\n  columns: 2,\n  [A], [c|],\n  [d], [e],\n)");
  assert.equal(
    run("#table(\n  columns: (1fr, 2cm, 3cm),\n  table.cell(colspan: 2)[A], [b],\n  [c], [d|], [e],\n)", act("delete-column")),
    "#table(\n  columns: (1fr, 3cm),\n  [A], [b],\n  [c], [e|],\n)",
  );
  // The only row / column: the whole figure goes, with its label.
  assert.equal(run("Text.\n\n#figure(\n  table(\n    columns: 2,\n    [a|], [b],\n  ),\n  caption: [],\n) <tab:1>\n\nAfter.", act("delete-row")), "Text.\n\n|After.");
  assert.equal(run("A.\n\n#table([a|], [b])\n\nB.", act("delete-column")), "A.\n\n|B.");
});

test("delete table: the figure with its label, else the call with its #", () => {
  assert.equal(run(sampleAt("[4]", 1), act("delete-table")), "Text.\n\n|After.");
  assert.equal(run("A.\n#table([a|])\nB.", act("delete-table")), "A.\n|B.");
  assert.equal(run("A.\n\n#table([a|])", act("delete-table")), "A.\n|");
  assert.equal(run("Inline #table([a|]) here.", act("delete-table")), "Inline | here.");
  // Not the figure's body: only the table.
  assert.equal(run("#figure(align(center, table([a|])), caption: [])", act("delete-table")), "#figure(align(center, |), caption: [])");
});

test("header row on and off", () => {
  assert.equal(run(sampleAt("[4]", 1), act("header")), SAMPLE.replace("    table.header([Night], [Ships], [Weather]),", "    [Night], [Ships], [Weather],").replace("[4]", "[4|]"));
  assert.equal(run("#table(\n  columns: 2,\n  [a|], [b],\n  [c], [d],\n)", act("header")), "#table(\n  columns: 2,\n  table.header([a|], [b]),\n  [c], [d],\n)");
  // A merged cell reaching below the first row can't be in the header.
  assert.equal(run("#table(columns: 2, table.cell(rowspan: 2)[a|], [b], [c])", act("header")), null);
  const header = tableMenu(stateAt(sampleAt("[4]", 1))).find((i) => i.type === "item" && i.label === "Header row");
  assert.ok(header && header.type === "item" && header.checked === true && header.act === "header");
});

test("merge cells: a rectangle of selected cells, contents joined; split gives empty cells back", () => {
  assert.equal(
    run(SAMPLE.replace("[Monday]", "[«Monday]").replace("[7]", "[7»]"), act("merge")),
    SAMPLE.replace("[Monday], [4], [Fog],\n    [Tuesday], [7], [Clear],", "table.cell(colspan: 2, rowspan: 2)[Monday 4 Tuesday 7|], [Fog],\n    [Clear],"),
  );
  // Empty cells add nothing; an expression joins as #(...); the first cell's other arguments stay.
  assert.equal(run("#table(columns: 3, table.cell(fill: red)[«a], [], str(1)»)", act("merge")), "#table(\n  columns: 3,\n  table.cell(fill: red, colspan: 3)[a #(str(1))|],\n)");
  // One cell, a rectangle cutting a merged cell, across header and body: no.
  assert.equal(run(sampleAt("[4]", 1), act("merge")), null);
  assert.equal(run("#table(columns: 3, [«a], table.cell(colspan: 2)[b], [c], [d»], [e])", act("merge")), null);
  assert.equal(run(SAMPLE.replace("[Night]", "[«Night]").replace("[Monday]", "[Monday»]"), act("merge")), null);
  // Split: plain again, empty cells in the freed slots.
  assert.equal(
    run("#table(\n  columns: 3,\n  table.cell(colspan: 2, rowspan: 2)[AB|], [c],\n  [f],\n  [g], [h], [i],\n)", act("split")),
    "#table(\n  columns: 3,\n  [AB|], [], [c],\n  [], [], [f],\n  [g], [h], [i],\n)",
  );
  assert.equal(run("#table(columns: 2, table.cell(colspan: 2, fill: red)[a|])", act("split")), "#table(\n  columns: 2,\n  table.cell(fill: red)[a|], [],\n)");
  assert.equal(run(sampleAt("[4]", 1), act("split")), null);
});

test("the menu: every command, those that don't apply greyed out, a fixed table's reason", () => {
  const items = tableMenu(stateAt(sampleAt("[4]", 1)));
  assert.deepEqual(
    items.map((i) => (i.type === "item" ? [i.label, i.act] : "-")),
    [
      ["Insert row above", "row-above"],
      ["Insert row below", "row-below"],
      ["Insert column left", "column-left"],
      ["Insert column right", "column-right"],
      "-",
      ["Delete row", "delete-row"],
      ["Delete column", "delete-column"],
      ["Delete table", "delete-table"],
      "-",
      ["Header row", "header"],
      ["Merge cells", null],
      ["Split cell", null],
    ],
  );
  assert.deepEqual(tableMenu(stateAt("Plain |text.")), []);
  const fixed = tableMenu(stateAt("#table(columns: 2, table.hline(), [a|], [b])"));
  const insert = fixed[0];
  assert.ok(insert.type === "item" && insert.act === null && /lines/.test(insert.title ?? ""));
  const del = fixed.find((i) => i.type === "item" && i.label === "Delete table");
  assert.ok(del && del.type === "item" && del.act === "delete-table");
});

test("an incomplete last row is filled with empty cells on an edit", () => {
  assert.equal(run("#table(columns: 3, [a], [b], [c], [d|])", act("row-below")), "#table(\n  columns: 3,\n  [a], [b], [c],\n  [d], [], [],\n  [|], [], [],\n)");
});
