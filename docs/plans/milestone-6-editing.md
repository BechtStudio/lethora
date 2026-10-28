# Milestone 6: Editing tools

Step plans, decisions, hand-test lists and known limitations for Milestone 6.

Planned with the user 2026-10-21, a track beside the collaboration hand
tests. Word-like tools in the author and science views; everything they
write is content markup, styles stay in the layout layer. Done and
hand-tested 2026-10-23.

- [x] 1. Formatting row (detailed 2026-10-21): a slim row under the title
      bar (author and science; hidden in the publisher view and focus mode;
      disabled while read only). Toggle buttons with active state: bold
      `*…*`, italic `_…_`, underline `#underline[…]`, strikethrough
      `#strike[…]`, superscript `#super[…]`, subscript `#sub[…]`; a heading
      box (Paragraph, Heading 1–6, `=` prefixes); bulleted `- ` and
      numbered `+ ` list toggles on the selected lines. Word behaviour: no
      selection inside a word formats the word, between words inserts an
      empty pair, inside a format removes it; a partial-word selection uses
      `#strong[…]`/`#emph[…]` (Typst ignores `*`/`_` inside words). Keys
      Ctrl+B/I/U; all in a new Format menu. The editor styles underline,
      strike, super and sub content like bold/italic (single line). Pure
      logic in `app/src/format.ts` with tests. Built 2026-10-21, checked in
      the dev mock, hand-tested by the user 2026-10-21 (`tauri dev`: Ctrl+B/I/U on a
      selection, a word, between words, inside bold; partial word →
      `#strong[…]`; S, x², x₂; heading box and both list buttons on several
      lines; undo each; Format menu; row hidden in Publisher and focus
      mode; read-only Reader sees it disabled). Known: lines are scanned
      from their start as markup (a line inside multi-line math/raw can be
      misread); calls with arguments (`#underline(offset: 2pt)[x]`) are
      styled but not toggled or shown pressed; a `]` in a code string inside
      styled content ends the styling early; Ctrl+I replaces CodeMirror's
      select-parent-syntax; headings/lists apply per line, not per Typst
      paragraph; the Format menu adds to the menu/view-switch overlap below
      ~420 px.
- [x] 2. Font and size (decided 2026-10-21, option B; detailed 2026-10-21;
      hand-tested 2026-10-21; the sync and Reader points on two PCs
      2026-10-23, which found Edit → Cut/Paste enabled while read only,
      fixed the same day).
      No selection: the row's font and size boxes show and set the body
      font/size (`text.font`, `text.size`). A selection: picking a font or
      size applies a character style from the layout layer (reused when an
      unnamed style with the same settings exists, else created on the
      spot); inside styled text a change moves only that text to another
      style; editing a style (all its uses) is done in the Styles list.
      Storage: flat keys in the layout map, `styles.<id>.font`,
      `styles.<id>.size` (absolute, like `text.size`), optional
      `styles.<id>.name` (named styles; unnamed ones show a name computed
      from their settings, "EB Garamond 14 pt", "Custom N" as fallback),
      `styles.<id>.removed` hides a preset's style. Ids are readable slugs
      that never change ("eb-garamond-14pt"). Content:
      `#styled("eb-garamond-14pt")[text]`; the preamble defines the styles
      and `styled`; a missing style falls back to body text. Presets can
      define named styles (mechanism only, none shipped yet). Deleting a
      used style is allowed (confirm with the use count; the text falls
      back, the editor marks the reference missing; undo restores). The
      author and science editors show styled text in its font at a size
      relative to the body (14 pt on 11 pt → ~1.27× the editor size).
      Parts:
  - [x] 2a. Core (typeset): styles collection (parse, validate, presets,
        removed), `layout_set` accepts `styles.*`, preamble `styled`,
        `LayoutState` carries the styles; dev mock; tests. Built and
        reviewed 2026-10-21 (nothing to hand-test without the UI of 2b).
        Stored values merge per field with a preset's style; a bad field is
        ignored alone, a style without font and size is dropped. Known: a
        lone `styles.<id>.name` is stored but unused, and stays after a
        style's last font/size is removed (2b/2c should write and remove
        whole styles); invalid style groups aren't reported; a preset
        style's field can be changed but not cleared; `#styled(5)[x]` (not
        a string) is a Typst error
  - [x] 2b. Row: font picker with previews in each font (embedded Typst
        fonts loaded into the web view), size box, Style dropdown (named
        first); apply/reuse/switch styles; `format.ts` reads
        `#styled(…)[…]`; editor styling of styled text, missing marked.
        Built and checked in the dev mock 2026-10-21 (pure parts in
        `styles.ts`, `fontpicker.ts`; `restyle` in `format.ts`; commands
        `layout_embedded_fonts`, `layout_font_file`); the row scrolls
        sideways (wheel too) below ~805 px, the boxes shrink below 860 px.
        Hand test with 2c. Known: creating a style and editing the text are
        two undo steps; styled spans fully inside a selection stay nested
        (not re-pointed); line-bounded like the step-1 formats; Escape in a
        box goes to an open drawer first; font and size aren't in the
        Format menu; embedded-font previews only in the real app (the mock
        has no font files)
  - [x] 2c. Layout panel "Character styles": add named, rename, edit font
        and size, delete (fallback), remove unused; the font picker of 2b
        replaces the panel's datalist. Built and checked in the dev mock
        2026-10-21; the row's Style box ends with "Edit styles…" (opens the
        Publisher tab at the section; the split pane hides the panel).
        Known: a preset style with every field changed looks user-made, so
        delete brings the preset's back (no preset ships styles); an open
        combobox list doesn't follow when the panel scrolls; the font box
        is squeezed in a very narrow panel; use counts rescan the whole text
        (debounced 400 ms) while the publisher shows.
        Hand test of step 2 (`tauri dev`): select a word → size 16 → bigger
        in the editor and on the pages; cursor in it → font EB Garamond
        (preview list shows each font, embedded ones too) → style "EB
        Garamond 16 pt", size kept; Style box → Default text unwraps; same
        settings on another word reuse the style; cursor in plain text →
        font/size change the whole document; undo each (style creation and
        text are two steps); Edit styles… → Add style, rename, change its
        font/size (all uses follow), Delete with uses (confirm, text falls
        back and is marked missing in the editor, undo restores), Remove
        unused; body Font row in Typography uses the new picker; a second
        PC (server sync) sees new styles and styled text; Reader: all
        disabled
- [x] 3. Tables (detailed 2026-10-21). Content: `#figure(table(…),
      caption: […]) <tab:N>`; table looks live in the layout layer.
  - [x] 3a. Insert and styles: a "Table" button in the formatting row (and
        Edit → Insert table…) opens a Word-like rows × columns grid picker
        (up to 10 × 8, "Header row" checkbox); inserts the table as its own
        block with the next free `<tab:N>`, cursor in the first cell.
        Layout settings (new "Tables" section): default table style
        (Plain, Academic = booktabs-like rules, Grid, Striped), bold header
        on/off, caption above/below; turned into Typst in the preamble.
        One table's different style is an anchored exception (new override
        kind like figure placement: Exceptions drawer, page marker,
        right-click on the table in the pages). User-defined table styles:
        later, if needed.
        Built and checked in the dev mock 2026-10-21. Decisions: preset
        defaults paper/thesis/manuscript Academic, caption above, bold
        header; novel Plain, caption below, bold header (keys
        `tables.style`, `tables.header_bold`, `tables.caption_position`;
        the figures caption setting now covers other figures). Inserted
        caption is empty; the picker's rows include the header row (3 rows
        + header = header and 2 body rows). Typst: each style is a set of
        `table` arguments (`lethora-tables.<style>`): the default as a set
        rule, an exception (`table_style` override) injects them into its
        table call (explicit arguments beat set rules; no style-dependent
        show rule, so nothing leaks). Academic's rule under the header and
        at the bottom are `table.hline`s injected at compile time into
        each academic table call (stroke functions can't see the last
        row). Bold header: a `show table` rule bolds the header rows only
        when the table's first item is a `table.header`. Clicks on the
        pages report `table_utf16`; the Exceptions drawer has a "Table"
        chip. Known: academic rules only in `table(...)` calls without
        trailing content blocks, a header spread in via `..` is missed,
        tables built by code get only the top rule; bold header also
        reaches tables nested in a cell, header rowspans not counted;
        striping by row index fills a second header row; a repeated
        header repeats the top rule, the bottom rule only at the table's
        end; exceptions are matched by position (as figure placement);
        a table split across pages is marked on its first page only; in
        the dev mock the fixture page's table can't be right-clicked (the
        submenu is covered by tests); the Table button re-opens an open
        picker; with an academic default every compile of a text
        containing `table(` parses the content once more.
        Hand-tested by the user 2026-10-22.
        Hand test (`tauri dev`): Table button and Edit → Insert table… →
        picker (hover, arrows, Enter, Esc, Header row off) → table on an
        empty line and mid-paragraph (lands after the paragraph), cursor
        in the first cell, next free `<tab:N>`, one undo; @tab:N reference;
        Publisher → Tables: each default style, bold header off, caption
        above/below (other figures unaffected); right-click a table →
        Table style → Grid (marker, drawer "Table" chip, popover change,
        Default removes it); preset switch to Novel gives Plain; Reader:
        button disabled
  - [x] 3b. Editing on the code: cursor in a table → Tab/Shift+Tab move
        between cells, insert row above/below, column left/right, delete
        row/column, merge cells (colspan/rowspan); from the row and a
        context menu, all as edits of the Typst text
        Detailed 2026-10-22: app only (`tables.ts`, pure and tested). The
        `table(...)` call at the cursor is read into a grid (`columns` as a
        number or a track list, `table.header`, positional cells, merged
        `table.cell(colspan:/rowspan:)`); after an edit the table is printed
        back one row per line, cell contents and named arguments kept as
        written, sent as the minimal changed span (an exception anchored
        at `table(` stays). Commands: Tab/Shift+Tab next/previous cell
        (selects its content; Tab in the last cell adds a row), insert row
        above/below, column left/right, delete row/column/table, header row
        on/off, merge selected cells (contents joined), split a merged
        cell; one undo step each; disabled outside a table and read only.
        UI: a "Table ▾" menu button beside the Table button while the
        cursor is in a table; an own editor right-click menu everywhere
        (Cut, Copy, Paste, plus the table items in a table), reusing the
        page menu's component.
        Built and checked in the dev mock 2026-10-22 (Tab/Shift+Tab, Table ▾,
        insert column, right-click merge, undo). Decisions: commands act
        on the rows/columns of the selected cells (like Word); a row
        inserted among header rows is a header row; deleting every row or
        column deletes the table; Delete table removes the whole
        `#figure(...)` with its label when the table is its body; merge
        needs a full rectangle of 2+ cells, all header or all body, no
        merged cell cut; split keeps other cell arguments (`fill:`); Tab
        is bound below the `@` completion's Tab and falls through outside
        a table. "Fixed" tables (lines, footer, x/y cells, comments
        between arguments, unreadable columns) allow Tab and Delete table
        only; the greyed items say why. Known: the editor's right-click
        menu replaces the web view's (its spelling suggestions are gone);
        named arguments after the cells move above them, a changed merged
        cell's arguments are reordered; a one-line table becomes one row
        per line on its first structural edit; `rows:` isn't updated; a
        structural command right after typing can share its undo step; a
        table nested in another call (`align(center, table(...))`) loses
        only the `table(...)` on Delete table; `table[...]` forms and
        spread arguments aren't editable.
        Hand-tested by the user 2026-10-22.
        Hand test (`tauri dev`): in a table Tab/Shift+Tab through header
        and body cells, Tab in the last cell adds a row; Table ▾ appears
        only in a table; insert row above/below (also on the header row),
        column left/right, delete row/column/table (the figure and label
        go); header row off/on; select two or four cells → right-click →
        Merge cells, then Split cell; each undoes in one step; an exception
        on the table survives edits (marker stays); right-click outside
        tables: Cut/Copy/Paste work; `@` completion's Tab still wins;
        Reader: commands disabled
  - Later: a visual grid editor in the science view (moved to TODO.md
    "Later" when the milestone closed, 2026-10-23)

