use std::fmt::Debug;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use lethora_core::protocol::members::Role;
use lethora_core::{
    DeviceId, Document, Entries, Error, Identity, MapName, ReviewId, ReviewItem, ReviewKind,
    SuggestionState, TextChange, UserId,
};
use serde_json::json;

/// Device `n`, used by user `n`.
fn who(n: u8) -> Identity {
    Identity {
        device: DeviceId::parse(&format!("{n:032x}")).unwrap(),
        user: UserId::parse(&format!("{:032x}", 0x100 + n as u32)).unwrap(),
    }
}

/// `review/<device>.<ext>` of identity `id` in `dir`.
fn review_file(dir: &Path, id: &Identity, ext: &str) -> PathBuf {
    dir.join("review")
        .join(format!("{}.{ext}", id.device.as_str()))
}

fn new_doc(text: &str) -> (tempfile::TempDir, Document) {
    let tmp = tempfile::tempdir().unwrap();
    let mut doc = Document::create(tmp.path(), &who(1)).unwrap();
    doc.insert(0, text).unwrap();
    (tmp, doc)
}

fn item(doc: &Document, id: ReviewId) -> ReviewItem {
    doc.review_items()
        .into_iter()
        .find(|i| i.id == id)
        .unwrap_or_else(|| panic!("no item {id:?}"))
}

/// The text of item `id`'s range.
fn under(doc: &Document, id: ReviewId) -> String {
    let i = item(doc, id);
    let units: Vec<u16> = doc.text().encode_utf16().collect();
    String::from_utf16(&units[i.from..i.to]).unwrap()
}

fn ch(from: usize, to: usize, insert: &str) -> TextChange {
    TextChange {
        from,
        to,
        insert: insert.into(),
    }
}

#[test]
fn anchors_follow_the_text() {
    let (tmp, mut doc) = new_doc("0123456789");
    let c = doc.add_comment(3, 6, "note").unwrap();
    assert_eq!(under(&doc, c), "345");
    // Just before and just after: outside.
    doc.insert(3, "ab").unwrap();
    doc.end_undo_step();
    doc.insert(8, "cd").unwrap();
    doc.end_undo_step();
    assert_eq!(doc.text(), "012ab345cd6789");
    assert_eq!(under(&doc, c), "345");
    // Inside: grows.
    doc.insert(6, "X").unwrap();
    assert_eq!(under(&doc, c), "3X45");
    // Undo and redo keep the anchors.
    doc.undo().unwrap();
    assert_eq!(under(&doc, c), "345");
    doc.redo().unwrap();
    assert_eq!(under(&doc, c), "3X45");
    // Reopening too.
    doc.autosave().unwrap();
    drop(doc);
    let mut doc = Document::open(tmp.path(), &who(1)).unwrap();
    assert_eq!(under(&doc, c), "3X45");
    // Deleting the last and the first character shrinks the range.
    doc.apply_changes(&[ch(8, 9, "")]).unwrap();
    assert_eq!(under(&doc, c), "3X4");
    doc.apply_changes(&[ch(5, 6, "")]).unwrap();
    assert_eq!(under(&doc, c), "X4");
    assert!(!item(&doc, c).detached);
    // Deleting all of it detaches the item; it stays listed where it was.
    assert_eq!(doc.text(), "012abX4cd6789");
    doc.end_undo_step();
    doc.apply_changes(&[ch(5, 7, "")]).unwrap();
    let i = item(&doc, c);
    assert!(i.detached);
    assert_eq!((i.from, i.to), (5, 5));
    // Undo brings the text back as new characters (as for `anchor_at`):
    // the item stays detached, at the restored text (known limitation).
    doc.undo().unwrap();
    assert_eq!(doc.text(), "012abX4cd6789");
    let i = item(&doc, c);
    assert!(i.detached && (5..=7).contains(&i.from), "{i:?}");
}

#[test]
fn deleting_around_the_range_detaches_it() {
    let (_tmp, mut doc) = new_doc("0123456789");
    let c = doc.add_mark(3, 6, "green").unwrap();
    doc.apply_changes(&[ch(1, 8, "")]).unwrap();
    let i = item(&doc, c);
    assert!(i.detached);
    assert_eq!((i.from, i.to), (1, 1));
    assert_eq!(i.kind, ReviewKind::Mark);
}

#[test]
fn empty_ranges_stay_points() {
    let (_tmp, mut doc) = new_doc("abc");
    let point = doc.add_comment(1, 1, "here").unwrap();
    let end = doc.add_comment(3, 3, "at the end").unwrap();
    doc.insert(3, "def").unwrap();
    doc.end_undo_step();
    let e = item(&doc, end);
    assert_eq!((e.from, e.to, e.detached), (6, 6, false));
    // A point stays in front of its character.
    doc.insert(1, "X").unwrap();
    let p = item(&doc, point);
    assert_eq!((p.from, p.to, p.detached), (2, 2, false));
    // Deleting that character never detaches a point.
    doc.apply_changes(&[ch(2, 3, "")]).unwrap();
    let p = item(&doc, point);
    assert_eq!((p.from, p.to, p.detached), (2, 2, false));
}

#[test]
fn utf16_positions() {
    let (_tmp, mut doc) = new_doc("a\u{1F600}b");
    let c = doc.add_comment(1, 3, "emoji").unwrap();
    assert_eq!(under(&doc, c), "\u{1F600}");
    assert!(matches!(
        doc.add_comment(2, 3, "x"),
        Err(Error::InvalidChange(_))
    ));
    assert!(matches!(
        doc.add_comment(3, 9, "x"),
        Err(Error::InvalidChange(_))
    ));
    assert!(matches!(
        doc.add_mark(2, 1, "pink"),
        Err(Error::InvalidChange(_))
    ));
    assert!(matches!(
        doc.add_mark(0, 1, "purple"),
        Err(Error::InvalidChange(_))
    ));
    assert!(matches!(
        doc.add_mark(1, 1, "pink"),
        Err(Error::InvalidChange(_))
    ));
}

#[test]
fn threads_marks_and_kinds() {
    let (_tmp, mut doc) = new_doc("Some text to review.");
    let mark = doc.add_mark(5, 9, "yellow").unwrap();
    let i = item(&doc, mark);
    assert_eq!(i.kind, ReviewKind::Mark);
    assert_eq!(i.colour.as_deref(), Some("yellow"));
    assert_eq!(i.author.as_ref().unwrap().user, who(1).user);
    assert_eq!(i.author.as_ref().unwrap().device, who(1).device);
    doc.set_mark_colour(mark, "blue").unwrap();
    // A first message makes a mark a comment, which keeps its colour.
    let first = doc.reply(mark, "Why?").unwrap();
    let i = item(&doc, mark);
    assert_eq!(i.kind, ReviewKind::Comment);
    assert_eq!(i.colour.as_deref(), Some("blue"));
    let second = doc.reply(mark, "Because.").unwrap();
    doc.edit_message(mark, first, "Why so?").unwrap();
    doc.delete_message(mark, second).unwrap();
    let m = item(&doc, mark).messages;
    assert_eq!(m.len(), 2);
    assert_eq!(
        (m[0].id, m[0].text.as_str(), m[0].deleted),
        (first, "Why so?", false)
    );
    assert!(m[0].edited.is_some() && m[0].created > 0);
    assert_eq!((m[1].text.as_str(), m[1].deleted), ("", true));
    assert_eq!(m[1].author.as_ref().unwrap().user, who(1).user);
    assert!(matches!(
        doc.edit_message(mark, second, "x"),
        Err(Error::InvalidChange(_))
    ));
    doc.resolve(mark).unwrap();
    assert!(item(&doc, mark).resolved);
    doc.reopen(mark).unwrap();
    assert!(!item(&doc, mark).resolved);
    doc.remove_item(mark).unwrap();
    assert!(doc.review_items().is_empty());
    assert!(matches!(doc.reply(mark, "x"), Err(Error::NotFound(_))));
    // Undoing the removal brings the same item back, author and all.
    assert!(doc.review_undo().unwrap());
    assert_eq!(item(&doc, mark).author.unwrap().user, who(1).user);
    // The listing serialises for the app.
    let json = serde_json::to_value(doc.review_items()).unwrap();
    assert_eq!(json[0]["kind"], json!("comment"));
    assert_eq!(json[0]["author"]["user"], json!(who(1).user.as_str()));
}

#[test]
fn suggest_mode_merges_typing() {
    let (_tmp, mut doc) = new_doc("The cat sat.");
    // Typing "big " before "cat" builds one insert suggestion.
    let s = doc.suggest_change(4, 4, "b").unwrap().unwrap();
    assert_eq!(doc.suggest_change(4, 4, "i").unwrap(), Some(s));
    assert_eq!(doc.suggest_change(4, 4, "g").unwrap(), Some(s));
    let i = item(&doc, s);
    let sug = i.suggestion.unwrap();
    assert_eq!((i.from, i.to), (4, 4));
    assert_eq!((sug.insert.as_str(), sug.original.as_str()), ("big", ""));
    assert_eq!(sug.state, SuggestionState::Open);
    // Backspacing eats the typed text, then the suggestion.
    assert_eq!(doc.suggest_change(3, 4, "").unwrap(), Some(s));
    assert_eq!(item(&doc, s).suggestion.unwrap().insert, "bi");
    assert_eq!(doc.suggest_change(3, 4, "").unwrap(), Some(s));
    assert_eq!(doc.suggest_change(3, 4, "").unwrap(), None);
    assert!(doc.review_items().is_empty());
    assert_eq!(doc.text(), "The cat sat.");

    // Backspacing over "cat" from its end (the app moves the caret).
    let d = doc.suggest_change(6, 7, "").unwrap().unwrap();
    assert_eq!(doc.suggest_change(5, 6, "").unwrap(), Some(d));
    assert_eq!(doc.suggest_change(4, 5, "").unwrap(), Some(d));
    // Deleting inside the range changes nothing.
    assert_eq!(doc.suggest_change(5, 6, "").unwrap(), Some(d));
    let i = item(&doc, d);
    assert_eq!((i.from, i.to), (4, 7));
    assert_eq!(i.suggestion.as_ref().unwrap().original, "cat");
    // Then typing "dog" at its end makes it a replacement.
    for c in ["d", "o", "g"] {
        assert_eq!(doc.suggest_change(7, 7, c).unwrap(), Some(d));
    }
    let sug = item(&doc, d).suggestion.unwrap();
    assert_eq!(
        (sug.insert.as_str(), sug.original.as_str(), sug.stale),
        ("dog", "cat", false)
    );
    // A backspace after typed text shrinks the insert.
    assert_eq!(doc.suggest_change(6, 7, "").unwrap(), Some(d));
    assert_eq!(item(&doc, d).suggestion.unwrap().insert, "do");
    // Deleting forward from its end widens the range.
    assert_eq!(doc.suggest_change(7, 8, "").unwrap(), Some(d));
    let i = item(&doc, d);
    let sug = i.suggestion.unwrap();
    assert_eq!((i.from, i.to), (4, 8));
    assert_eq!((sug.insert.as_str(), sug.original.as_str()), ("do", "cat "));
    // Typing elsewhere starts a new suggestion.
    let other = doc.suggest_change(11, 11, "!").unwrap().unwrap();
    assert_ne!(other, d);
    // Replacing over the range's start widens it and appends.
    assert_eq!(doc.suggest_change(3, 5, "X").unwrap(), Some(d));
    let i = item(&doc, d);
    let sug = i.suggestion.unwrap();
    assert_eq!((i.from, i.to), (3, 8));
    assert_eq!(
        (sug.insert.as_str(), sug.original.as_str()),
        ("doX", " cat ")
    );
    assert_eq!(doc.text(), "The cat sat.");
    assert_eq!(doc.review_items().len(), 2);
    // A burst of suggestions is one review undo step.
    assert!(doc.review_undo().unwrap());
    assert!(doc.review_items().is_empty());
    assert!(!doc.can_review_undo());
}

#[test]
fn suggestions_merge_only_into_own_open_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let mut a = Document::create(tmp.path(), &who(1)).unwrap();
    a.insert(0, "One two three.").unwrap();
    let mine = a.suggest_change(3, 3, "x").unwrap().unwrap();
    a.autosave().unwrap();
    let mut b = Document::open(tmp.path(), &who(2)).unwrap();
    let theirs = b.suggest_change(3, 3, "y").unwrap().unwrap();
    assert_ne!(theirs, mine);
    // Rejected or resolved ones take nothing more.
    a.reject_suggestion(mine).unwrap();
    a.end_undo_step();
    let next = a.suggest_change(3, 3, "z").unwrap().unwrap();
    assert_ne!(next, mine);
    a.resolve(next).unwrap();
    let again = a.suggest_change(3, 3, "w").unwrap().unwrap();
    assert_ne!(again, next);
    assert_eq!(a.suggest_change(3, 3, "").unwrap(), None);
}

#[test]
fn accept_reject_stale_detached() {
    let (_tmp, mut doc) = new_doc("The cat sat.");
    let s = doc.suggest_change(4, 7, "dog").unwrap().unwrap();
    doc.end_undo_step();
    let changes = doc.accept_suggestion(s).unwrap();
    assert_eq!(changes, vec![ch(4, 7, "dog")]);
    assert_eq!(doc.text(), "The dog sat.");
    assert_eq!(
        item(&doc, s).suggestion.unwrap().state,
        SuggestionState::Accepted
    );
    assert!(matches!(
        doc.accept_suggestion(s),
        Err(Error::InvalidChange(_))
    ));
    assert!(matches!(
        doc.reject_suggestion(s),
        Err(Error::InvalidChange(_))
    ));
    // One main undo step; it doesn't reopen the suggestion (known).
    doc.undo().unwrap();
    assert_eq!(doc.text(), "The cat sat.");
    assert_eq!(
        item(&doc, s).suggestion.unwrap().state,
        SuggestionState::Accepted
    );

    // An insertion and a deletion.
    doc.end_undo_step();
    let ins = doc.suggest_change(12, 12, " Done.").unwrap().unwrap();
    doc.end_undo_step();
    let del = doc.suggest_change(0, 4, "").unwrap().unwrap();
    doc.accept_suggestion(ins).unwrap();
    doc.accept_suggestion(del).unwrap();
    assert_eq!(doc.text(), "cat sat. Done.");

    // Rejecting leaves main alone.
    let r = doc.suggest_change(0, 3, "Dog").unwrap().unwrap();
    doc.reject_suggestion(r).unwrap();
    assert_eq!(
        item(&doc, r).suggestion.unwrap().state,
        SuggestionState::Rejected
    );
    assert_eq!(doc.text(), "cat sat. Done.");

    // Stale: the text under it changed.
    doc.end_undo_step();
    let stale = doc.suggest_change(4, 7, "lay").unwrap().unwrap();
    doc.insert(5, "a").unwrap();
    assert!(item(&doc, stale).suggestion.unwrap().stale);
    assert!(matches!(doc.accept_suggestion(stale), Err(Error::Stale(_))));
    doc.undo().unwrap();
    assert!(!item(&doc, stale).suggestion.unwrap().stale);

    // Detached: the text under it is gone.
    doc.apply_changes(&[ch(4, 7, "")]).unwrap();
    let i = item(&doc, stale);
    assert!(i.detached && i.suggestion.unwrap().stale);
    assert!(matches!(doc.accept_suggestion(stale), Err(Error::Stale(_))));
    // A comment is not a suggestion.
    let c = doc.add_comment(0, 1, "x").unwrap();
    assert!(matches!(
        doc.accept_suggestion(c),
        Err(Error::InvalidChange(_))
    ));
}

#[test]
fn review_and_main_undo_are_separate() {
    let (_tmp, mut doc) = new_doc("Text");
    doc.end_undo_step();
    let c = doc.add_comment(0, 4, "first").unwrap();
    let reply = doc.reply(c, "second").unwrap();
    // Each entry is a step of its own.
    assert!(doc.review_undo().unwrap());
    assert!(item(&doc, c).messages.iter().all(|m| m.id != reply));
    assert_eq!(item(&doc, c).messages.len(), 1);
    assert_eq!(doc.text(), "Text");
    assert!(doc.can_undo());
    // Main undo leaves review alone.
    doc.undo().unwrap();
    assert_eq!(doc.text(), "");
    assert_eq!(doc.review_items().len(), 1);
    assert!(doc.review_redo().unwrap());
    assert_eq!(item(&doc, c).messages.len(), 2);
    assert!(doc.review_undo().unwrap());
    assert!(doc.review_undo().unwrap());
    assert!(doc.review_items().is_empty());
    assert!(!doc.review_undo().unwrap());
    assert!(doc.review_redo().unwrap());
    assert_eq!(doc.review_items().len(), 1);
    assert!(doc.can_redo());
    doc.redo().unwrap();
    assert_eq!(doc.text(), "Text");
    // `end_undo_step` splits suggestion bursts.
    doc.suggest_change(4, 4, "a").unwrap();
    doc.end_undo_step();
    doc.suggest_change(4, 4, "b").unwrap();
    doc.review_undo().unwrap();
    let s = doc
        .review_items()
        .into_iter()
        .find_map(|i| i.suggestion)
        .unwrap();
    assert_eq!(s.insert, "a");
}

#[test]
fn restore_leaves_review_alone() {
    let (_tmp, mut doc) = new_doc("Before");
    let cp = doc.checkpoint(Some("cp"), SystemTime::now()).unwrap();
    let c = doc.add_comment(0, 6, "keep me").unwrap();
    doc.insert(6, " and after").unwrap();
    doc.restore(cp).unwrap();
    assert_eq!(doc.text(), "Before");
    assert_eq!(under(&doc, c), "Before");
    assert_eq!(doc.review_items().len(), 1);
    // The checkpoint's text is main's alone.
    assert_eq!(doc.text_at(cp).unwrap(), "Before");
}

#[test]
fn review_persists_compacts_and_repairs() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let me = who(1);
    let mut doc = Document::create(dir, &me).unwrap();
    doc.insert(0, "Persistent text").unwrap();
    doc.autosave().unwrap();
    // Nothing is written for review until it changes.
    assert!(!dir.join("review").exists());
    drop(doc);
    let mut doc = Document::open(dir, &me).unwrap();
    assert!(doc.review_items().is_empty());
    doc.autosave().unwrap();
    assert!(!dir.join("review").exists());

    let c = doc.add_comment(0, 10, "saved").unwrap();
    doc.autosave().unwrap();
    let log = review_file(dir, &me, "log");
    let len = fs::metadata(&log).unwrap().len();
    assert!(len > 0);
    // A no-op autosave writes nothing.
    doc.autosave().unwrap();
    assert_eq!(fs::metadata(&log).unwrap().len(), len);
    let listed = doc.review_items();
    drop(doc);

    // A torn tail is cut away on open.
    OpenOptions::new()
        .append(true)
        .open(&log)
        .unwrap()
        .write_all(&[9, 0, 0, 0, 1, 2])
        .unwrap();
    let mut doc = Document::open(dir, &me).unwrap();
    assert_eq!(doc.review_items(), listed);
    assert_eq!(fs::metadata(&log).unwrap().len(), len);

    // Compaction writes a snapshot and empties the log.
    doc.set_compaction_threshold(1);
    doc.reply(c, "compacted").unwrap();
    doc.autosave().unwrap();
    assert_eq!(fs::metadata(&log).unwrap().len(), 0);
    assert!(review_file(dir, &me, "snapshot").exists());
    let listed = doc.review_items();
    // Dropping autosaves review too.
    doc.set_compaction_threshold(u64::MAX);
    doc.resolve(c).unwrap();
    let resolved = doc.review_items();
    assert_ne!(resolved, listed);
    drop(doc);
    let doc = Document::open(dir, &me).unwrap();
    assert_eq!(doc.review_items(), resolved);
    assert_eq!(doc.text(), "Persistent text");
}

#[test]
fn two_devices_share_review_items() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut a = Document::create(dir, &who(1)).unwrap();
    a.insert(0, "Shared text with words.").unwrap();
    let c = a.add_comment(7, 11, "from A").unwrap();
    a.autosave().unwrap();
    // Opening reads the other device's review files.
    let mut b = Document::open(dir, &who(2)).unwrap();
    let i = item(&b, c);
    assert_eq!(i.author.as_ref().unwrap().user, who(1).user);
    assert_eq!(i.messages[0].text, "from A");
    let reply = b.reply(c, "from B").unwrap();
    let mark = b.add_mark(0, 6, "pink").unwrap();
    b.autosave().unwrap();
    let p = a.pull_local().unwrap();
    assert!(p.review && !p.changed && p.skipped.is_empty());
    let i = item(&a, c);
    assert_eq!(i.messages[1].id, reply);
    let author = i.messages[1].author.clone().unwrap();
    assert_eq!((author.user, author.device), (who(2).user, who(2).device));
    assert_eq!(item(&a, mark).author.unwrap().user, who(2).user);
    // Nothing new: nothing reported.
    assert!(!a.pull_local().unwrap().review);
    // Only the author edits; the owner (role None) may delete.
    assert!(matches!(
        a.edit_message(c, reply, "x"),
        Err(Error::NotAuthor(_))
    ));
    assert!(matches!(
        a.set_mark_colour(mark, "blue"),
        Err(Error::NotAuthor(_))
    ));
    a.delete_message(c, reply).unwrap();

    // A concurrent edit inside the range on another device grows it.
    b.insert(9, "XY").unwrap();
    b.autosave().unwrap();
    let p = a.pull_local().unwrap();
    assert!(p.changed && !p.review);
    assert_eq!(a.text(), "Shared teXYxt with words.");
    assert_eq!(under(&a, c), "teXYxt");
    a.autosave().unwrap();
    let p = b.pull_local().unwrap();
    assert!(p.review);
    assert!(item(&b, c).messages[1].deleted);
}

/// The name [`Error::ReadOnly`] gives a role.
fn role_name(role: Option<Role>) -> &'static str {
    match role {
        None | Some(Role::Owner) => "Owner",
        Some(Role::Editor) => "Editor",
        Some(Role::Reviewer) => "Reviewer",
        Some(Role::Reader) => "Reader",
    }
}

/// Asserts `r` succeeded if `allowed`, else failed as read only for `role`.
fn rights<T: Debug>(what: &str, role: Option<Role>, allowed: bool, r: lethora_core::Result<T>) {
    if allowed {
        assert!(r.is_ok(), "{what} as {role:?}: {r:?}");
    } else {
        assert!(
            matches!(&r, Err(Error::ReadOnly(name)) if name == role_name(role)),
            "{what} as {role:?}: {r:?}"
        );
    }
}

/// Like [`rights`], with `author` deciding between success and
/// [`Error::NotAuthor`] once the role may write `review`.
fn author<T: Debug>(
    what: &str,
    role: Option<Role>,
    writable: bool,
    author: bool,
    r: lethora_core::Result<T>,
) {
    if writable && !author {
        assert!(
            matches!(r, Err(Error::NotAuthor(_))),
            "{what} as {role:?}: {r:?}"
        );
    } else {
        rights(what, role, writable, r);
    }
}

#[test]
fn role_matrix() {
    for role in [
        None,
        Some(Role::Owner),
        Some(Role::Editor),
        Some(Role::Reviewer),
        Some(Role::Reader),
    ] {
        let main = matches!(role, None | Some(Role::Owner | Role::Editor));
        let review = role != Some(Role::Reader);
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        // Another user's items.
        let mut a = Document::create(dir, &who(1)).unwrap();
        a.insert(0, "0123456789 abcdefghij klmnopqrst uvwxyz")
            .unwrap();
        let their_comment = a.add_comment(0, 2, "theirs").unwrap();
        let their_message = item(&a, their_comment).messages[0].id;
        let their_mark = a.add_mark(3, 5, "yellow").unwrap();
        let their_mark2 = a.add_mark(6, 8, "green").unwrap();
        let their_suggestion = a.suggest_change(11, 13, "AB").unwrap().unwrap();
        a.end_undo_step();
        let their_suggestion2 = a.suggest_change(14, 15, "C").unwrap().unwrap();
        a.autosave().unwrap();
        drop(a);
        // This device's own items, made while it could write.
        let mut b = Document::open(dir, &who(2)).unwrap();
        let comment = b.add_comment(22, 24, "mine").unwrap();
        let message = item(&b, comment).messages[0].id;
        let mark = b.add_mark(25, 27, "blue").unwrap();
        let mark2 = b.add_mark(28, 30, "blue").unwrap();
        let suggestion = b.suggest_change(33, 34, "Z").unwrap().unwrap();
        b.insert(39, "!").unwrap();
        let cp = b.checkpoint(Some("cp"), SystemTime::now()).unwrap();
        b.set_role(role);
        assert_eq!(b.is_writable(), main, "{role:?}");
        assert_eq!(b.is_review_writable(), review, "{role:?}");

        // Every entry that changes main.
        let r = role;
        let mut layout = Entries::new();
        layout.insert("columns".into(), json!(2));
        rights("insert", r, main, b.insert(0, "x"));
        rights("delete", r, main, b.delete(0, 1));
        rights("apply_changes", r, main, b.apply_changes(&[ch(0, 0, "y")]));
        rights(
            "map_replace",
            r,
            main,
            b.map_replace(MapName::Layout, &layout),
        );
        rights("set_custom_text", r, main, b.set_custom_text("#x"));
        rights("set_bibliography", r, main, b.set_bibliography("@a{b}"));
        rights("put_asset", r, main, b.put_asset("assets/a.png", b"a"));
        rights("stage_blob", r, main, b.stage_blob(b"s"));
        rights("add_asset_folder", r, main, b.add_asset_folder("assets/f/"));
        rights(
            "insert_with_assets",
            r,
            main,
            b.insert_with_assets(0, "z", &[]),
        );
        rights(
            "repoint_asset",
            r,
            main,
            b.repoint_asset(0, 0, "", "assets/b.png", b"b"),
        );
        let moves = [("assets/a.png".to_owned(), "assets/c.png".to_owned())];
        rights("move_assets", r, main, b.move_assets(&moves));
        rights(
            "remove_assets",
            r,
            main,
            b.remove_assets(&["assets/f/".to_owned()]),
        );
        rights("undo", r, main, b.undo());
        rights("redo", r, main, b.redo());
        rights("restore", r, main, b.restore(cp));
        rights(
            "named checkpoint",
            r,
            main,
            b.checkpoint(Some("n"), SystemTime::now()),
        );

        // Every review entry, on own items.
        rights("add_comment", r, review, b.add_comment(0, 1, "new"));
        rights("reply", r, review, b.reply(comment, "again"));
        rights(
            "edit_message",
            r,
            review,
            b.edit_message(comment, message, "e"),
        );
        rights(
            "delete_message",
            r,
            review,
            b.delete_message(comment, message),
        );
        rights("resolve", r, review, b.resolve(comment));
        rights("reopen", r, review, b.reopen(comment));
        rights("add_mark", r, review, b.add_mark(0, 1, "pink"));
        rights(
            "set_mark_colour",
            r,
            review,
            b.set_mark_colour(mark, "pink"),
        );
        rights("remove_item", r, review, b.remove_item(mark2));
        rights("suggest_change", r, review, b.suggest_change(36, 36, "q"));
        rights(
            "reject_suggestion",
            r,
            review,
            b.reject_suggestion(suggestion),
        );
        rights("review_undo", r, review, b.review_undo());
        rights("review_redo", r, review, b.review_redo());
        // And on the other user's.
        let (w, moderator) = (review, main);
        rights("reply theirs", r, review, b.reply(their_comment, "re"));
        rights("resolve theirs", r, review, b.resolve(their_comment));
        rights("reopen theirs", r, review, b.reopen(their_comment));
        author(
            "edit theirs",
            r,
            w,
            false,
            b.edit_message(their_comment, their_message, "e"),
        );
        author(
            "recolour theirs",
            r,
            w,
            false,
            b.set_mark_colour(their_mark, "pink"),
        );
        author(
            "delete theirs",
            r,
            w,
            moderator,
            b.delete_message(their_comment, their_message),
        );
        author("remove theirs", r, w, moderator, b.remove_item(their_mark2));
        author(
            "reject theirs",
            r,
            w,
            moderator,
            b.reject_suggestion(their_suggestion2),
        );
        // Accepting needs main.
        let text = b.text();
        rights("accept", r, main, b.accept_suggestion(their_suggestion));
        assert_eq!(b.text() != text, main, "{role:?}");
        // Reading always works.
        assert!(!b.review_items().is_empty());
        b.autosave().unwrap();
    }
}
