# Known limitations

These are things we know about and have accepted for now. Most are small or
rare, and some have a simple way around them. If you run into something that
isn't listed here, please report it in the
[issue tracker](https://github.com/BechtStudio/lethora/issues).

## Sharing and sync
- If the bibliography file is changed outside Lethora while the project is open on two devices, the change can be added twice. <!-- M5 1d -->
- The "Last sync" time in the sync dialog doesn't update while the dialog is open; close and reopen it to see the current time. <!-- M5 2b -->
- When you open a shared project, Lethora looks for a copy already on this computer only among open, recent and default-folder projects. <!-- M5 2d -->
- On Windows, a disconnected network drive can make the sync dialog slow to open. <!-- M5 2d -->
- If a shared project's key is missing on this device, the project says so, but there is no way to enter the key yet. <!-- M5 3c1 -->
- Images larger than the server allows aren't uploaded; they stay on this device and a status message says so. <!-- M5 3c1 -->
- With a large project, sharing and syncing with a server can pause typing for a moment. <!-- M5 3c1 -->
- Lethora remembers at most five servers. <!-- M5 3c1 -->
- If the server has lost the project, or you connect to the wrong server, the project looks deleted and your copy becomes a local project. <!-- M5 3c2 -->
- Deleting a project from the server needs your computer's clock to be right to within five minutes. <!-- M5 3c2 -->
- A shared project that became local keeps using your identity from the shared project until you open it again. <!-- M5 3c2 -->
- "Reopen from server" lists only projects this device remembers and still has the key for. <!-- M5 3c2 -->
- If the connection drops just as you create an invite, you see an error and the invite link isn't fetched again. <!-- M5 3f+ -->

## Members and roles
- A member change made while offline shows a timeout after 30 seconds, but it still goes through when you are back online. <!-- M5 3c2 -->
- If your role changes while you type, the last few keystrokes before the change are discarded. <!-- M5 3c2 -->
- If you are made read-only, edits not yet sent stay on your device until you can edit again. <!-- M5 3c2 -->
- While you are read-only, changes to the bibliography file made outside Lethora aren't picked up. <!-- M5 3c2 -->

## Seeing other writers
- Times in "Show authors" are approximate: changes made within about a quarter of an hour share one time. <!-- M5 1a, 1b -->
- A writer who disconnects stays visible for up to 30 seconds; after you reconnect, others can take up to 10 seconds to show again. <!-- M5 3e -->
- Another writer's cursor moves right when someone types exactly where it is, and a cursor in text you haven't received yet shows once that text arrives. <!-- M5 3e -->
- A writer's name flag plays again when its line scrolls back into view. <!-- M5 3e -->
- Another writer's cursor can lag one character behind while they type and stay there until their next edit. <!-- M5 3e -->
- "Show authors" counts text brought back by undo or a checkpoint as written by whoever brought it back. <!-- M5 3f -->
- "Show authors" leaves new typing uncoloured for a moment, and its legend sits under open drawers. <!-- M5 3f -->
- Two members can end up with the same colour, and you can pick a colour another member already has. <!-- M5 3f+ -->

## Comments and suggestions
- Undoing the change from an accepted suggestion doesn't reopen the suggestion. <!-- M5 4a -->
- In Suggest mode, typing inside your own suggested insertion adds the text at its end. <!-- M5 4a -->
- Times on cards ("5 min ago") update only when the panel is redrawn. <!-- M5 4c1 -->
- The panel catches up half a second after you edit: a comment whose text you just deleted shows as detached only then, and typing at a comment's edge counts as inside it until then. <!-- M5 4c1 -->
- In Suggest mode, Find's Replace switches between replacing and selecting the next match; use Replace all instead. <!-- M5 4c2 -->
- In Suggest mode, typing at the start of your own suggested deletion makes a new insertion instead of extending the old one. <!-- M5 4c2 -->
- In Suggest mode, formatting buttons don't put the cursor inside the markup they suggest. <!-- M5 4c2 -->
- With the panel's filter on Resolved, new suggestions you type don't show. <!-- M5 4c2 -->
- A reviewer who becomes an editor starts with Suggest changes off. <!-- M5 4c2 -->

## Checkpoints and bibliography
- Typing right after you make a named checkpoint starts a new undo step. <!-- M5 1c -->
- The checkpoint list doesn't show who made a checkpoint. <!-- M5 1c -->
- The Sources drawer still calls the bibliography "references.bib". <!-- M5 1d -->

## Images
- The figure preview in the science view has no right-click menu; replace an image from the pages instead. <!-- M5 1e -->
- "Replace image…" finds the file only when its path is on the same line as `image(`. <!-- M5 1e -->
- Restoring a checkpoint brings back images you have since moved or removed in the assets panel. <!-- M5 A1 -->
- The assets panel selects one file at a time, and its list view shows icons, not thumbnails. <!-- M5 A2 -->
- In the assets panel, the keyboard can leave the large preview, and Esc in the search box leaves the panel. <!-- M5 A2 -->
- Zoom keys don't reach the pages while the assets panel has focus. <!-- M5 A2 -->
- Figure previews reload after every step in the assets panel. <!-- M5 A2 -->
- Images used in the text but missing from the project, like a missing `sketch.png`, aren't listed in the assets panel. <!-- M5 A2 -->
- The Figure button always imports into `assets/`, not the folder open in the assets panel. <!-- M5 A3 -->
- A dropped image lands only at a blank line or at the start or end of the text, not between a heading and the text after it. <!-- M5 A3 -->
- Dropping two identical files at once adds two copies. <!-- M5 A3 -->

## Writing and formatting
- Pressing Enter inside a quote or caption written as markup (`#quote[…]`) starts a new paragraph there. <!-- M4 batch 0 -->
- Styling with extra settings, like `#underline(offset: 2pt)[x]`, shows, but its button can't turn it off and doesn't show as pressed. <!-- M6 1 -->
- Heading and list buttons work line by line, not on the whole paragraph. <!-- M6 1 -->

## Character styles
- A field of a built-in style can be changed but not cleared. <!-- M6 2a -->
- Creating a style and changing the text are two undo steps. <!-- M6 2b -->
- A style can't span several lines, and styled text fully inside your selection keeps its own style inside the new one. <!-- M6 2b -->
- Esc in the font or size box closes an open drawer first. <!-- M6 2b -->
- Font and size aren't in the Format menu. <!-- M6 2b -->
- A built-in style with every field changed looks like your own, so deleting it brings the built-in one back. <!-- M6 2c -->
- An open font list doesn't move along when the layout panel scrolls. <!-- M6 2c -->
- The font box gets cramped in a very narrow layout panel. <!-- M6 2c -->

## Tables
- Striped rows also colour a second header row. <!-- M6 3a -->
- A header repeated on the next page repeats the academic style's top line, and the bottom line shows only at the table's end. <!-- M6 3a -->
- A table split across pages is marked on its first page only. <!-- M6 3a -->
- The Table button opens its picker again even when it is already open. <!-- M6 3a -->
- A table written on one line is spread out to one row per line on its first row or column edit. <!-- M6 3b -->
- A table command right after typing can be undone together with the typing. <!-- M6 3b -->
- Tables with lines, a footer, cells placed by position or comments in their code allow only Tab and Delete table. <!-- M6 3b -->

## Spell check
- Spelling suggestions don't include sound-alike words. <!-- R2 -->
- In German, a shortened word like "geht's" counts as correct when its parts are. <!-- R2 -->
- Markup inside a word (like `*Haus*tür`) splits it for the check; hyphenated words are checked and marked as a whole. <!-- R2 -->

## Import and export
- Markdown import: a footnote referenced twice is duplicated. <!-- R3a -->
- Markdown import: column alignment, definition lists, superscript and subscript are lost. <!-- R3a -->
- Word import: a paragraph without a number inside a list item ends the list, and tables inside tables are flattened. <!-- R3b -->
- Word import: Wingdings and other symbol fonts (except Symbol) show as boxes. <!-- R3b -->
- Export: the bibliography becomes a bulleted list, and small caps and highlights keep only their text. <!-- R4a -->
- Export: links to labels become plain text, and citations don't link to their bibliography entry. <!-- R4a -->
- Export: an equation set to have no number still gets one when the layout numbers equations. <!-- R4a -->
- File → Export… has no keyboard shortcut (Ctrl+Shift+E is View → Markers). <!-- R4a -->
- EPUB: two books with the same title and authors get the same book identifier. <!-- R4b -->
- Word export: every table uses Word's plain grid style, and cross-references are links that don't update when numbers change. <!-- R4c -->

## Window and keyboard
- Other search keys, such as F3, Ctrl+G and Ctrl+D, do nothing. <!-- M5 T1 -->
- Ctrl+Shift+F and Ctrl+Shift+T do nothing until a project is open. <!-- M5 T1 -->
- On Windows the maximize button shows no tooltip. <!-- M5 T2 -->
- On macOS the window may show briefly without a frame at start, and the top row can be dragged but has no window buttons of its own. <!-- M5 T2 -->
- The recent list notices a moved or deleted project only when you open or close a project or restart. <!-- M5 T2 -->
- The window buttons can't be reached with the keyboard, as in most apps. <!-- M5 T2 -->
- With "Reopen the last project" off, Lethora starts on the start screen after an update. <!-- R7a -->
- Window-wide shortcuts still work behind the Settings view, as they do behind other dialogs. <!-- R7a -->
- Esc on a shortcut's "used by" line closes Settings instead of only the line. <!-- R7b -->
- On macOS, Settings shows Exit as ⌥F4 (the menu has Quit ⌘Q); a key the menu can't show, like Ctrl+ß, gives an error once, but the key still works. <!-- R7b -->
- In a very narrow window (under about 420 px), the menus overlap the view switch. <!-- M6 1 -->

## Layout
- After you click on a page, the editor has focus: click the pages once more before using the Ctrl zoom keys. <!-- M4 E -->
- The Exceptions drawer closes when the pages move between a tab and the split pane. <!-- M4 G -->
