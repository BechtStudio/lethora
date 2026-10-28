use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lethora_core::{
    Authorship, Checkpoint, CheckpointId, DeviceId, Document, Entries, Error, Identity,
    MAX_BIBLIOGRAPHY_BYTES, MapName, Recovered, TextChange, UserId,
};
use loro::{ExportMode, Frontiers, LoroDoc, LoroValue};
use serde_json::{Value, json};

/// Device `n`, used by user `n`.
fn who(n: u8) -> Identity {
    Identity {
        device: DeviceId::parse(&format!("{n:032x}")).unwrap(),
        user: UserId::parse(&format!("{:032x}", 0x100 + n as u32)).unwrap(),
    }
}

/// The identity most tests use.
fn me() -> Identity {
    who(1)
}

fn create(dir: &Path) -> lethora_core::Result<Document> {
    Document::create(dir, &me())
}

fn open(dir: &Path) -> lethora_core::Result<Document> {
    Document::open(dir, &me())
}

/// `main/<device>.<ext>` in `dir`.
fn own(dir: &Path, id: &Identity, ext: &str) -> PathBuf {
    dir.join("main")
        .join(format!("{}.{ext}", id.device.as_str()))
}

fn log_path(dir: &Path) -> PathBuf {
    own(dir, &me(), "log")
}

fn log_len(dir: &Path) -> u64 {
    fs::metadata(log_path(dir)).unwrap().len()
}

fn new_doc() -> (tempfile::TempDir, Document) {
    let tmp = tempfile::tempdir().unwrap();
    let doc = create(tmp.path()).unwrap();
    (tmp, doc)
}

/// A log record: `[u32 LE len][u32 LE crc32][payload]`.
fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = (payload.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(&crc32fast::hash(payload).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_700_000_000 + secs)
}

fn ch(from: usize, to: usize, insert: &str) -> TextChange {
    TextChange {
        from,
        to,
        insert: insert.into(),
    }
}

/// Applies `changes` to `text` with CodeMirror semantics (UTF-16 offsets into
/// the text before the change set), as the editor will.
fn apply_cm(text: &str, changes: &[TextChange]) -> String {
    let old: Vec<u16> = text.encode_utf16().collect();
    let mut out = Vec::with_capacity(old.len());
    let mut pos = 0;
    for c in changes {
        assert!(
            pos <= c.from && c.from <= c.to && c.to <= old.len(),
            "{changes:?}"
        );
        out.extend_from_slice(&old[pos..c.from]);
        out.extend(c.insert.encode_utf16());
        pos = c.to;
    }
    out.extend_from_slice(&old[pos..]);
    String::from_utf16(&out).unwrap()
}

/// Deterministic xorshift PRNG: `next(n)` is in `0..n`.
fn prng(mut state: u64) -> impl FnMut(usize) -> usize {
    move |n| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % n as u64) as usize
    }
}

#[test]
fn create_and_open_errors() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(matches!(open(tmp.path()), Err(Error::NotFound(_))));
    let doc = create(tmp.path()).unwrap();
    drop(doc);
    assert!(matches!(create(tmp.path()), Err(Error::AlreadyExists(_))));
    assert!(open(tmp.path()).unwrap().is_empty());
}

#[test]
fn insert_delete_text() {
    let (_tmp, mut doc) = new_doc();
    doc.insert(0, "Hello world").unwrap();
    doc.insert(5, ",").unwrap();
    assert_eq!(doc.text(), "Hello, world");
    doc.delete(0, 7).unwrap();
    assert_eq!(doc.text(), "world");
    assert_eq!(doc.len(), 5);
    doc.insert(5, "").unwrap();
    doc.delete(2, 0).unwrap();
    assert_eq!(doc.text(), "world");
}

#[test]
fn unicode_positions_are_scalar_indices() {
    let (_tmp, mut doc) = new_doc();
    doc.insert(0, "héllo 🌍 wörld").unwrap();
    assert_eq!(doc.len(), 13);
    doc.delete(6, 1).unwrap(); // the emoji
    assert_eq!(doc.text(), "héllo  wörld");
    doc.insert(6, "🎉").unwrap();
    doc.insert(doc.len(), "∑").unwrap();
    assert_eq!(doc.text(), "héllo 🎉 wörld∑");
    assert_eq!(doc.len(), 14);
}

#[test]
fn out_of_range_is_an_error() {
    let (_tmp, mut doc) = new_doc();
    doc.insert(0, "abc").unwrap();
    assert!(matches!(doc.insert(4, "x"), Err(Error::OutOfRange { .. })));
    assert!(matches!(doc.delete(1, 3), Err(Error::OutOfRange { .. })));
    assert!(matches!(doc.delete(4, 0), Err(Error::OutOfRange { .. })));
    assert!(matches!(
        doc.delete(usize::MAX, 2),
        Err(Error::OutOfRange { .. })
    ));
    assert_eq!(doc.text(), "abc");
}

#[test]
fn undo_redo_basics() {
    let (_tmp, mut doc) = new_doc();
    assert!(!doc.can_undo());
    assert!(doc.undo().unwrap().is_none());
    doc.insert(0, "one").unwrap();
    doc.end_undo_step();
    doc.insert(3, " two").unwrap();
    assert!(doc.undo().unwrap().is_some());
    assert_eq!(doc.text(), "one");
    assert!(doc.can_redo());
    assert!(doc.redo().unwrap().is_some());
    assert_eq!(doc.text(), "one two");
    doc.undo().unwrap();
    doc.undo().unwrap();
    assert_eq!(doc.text(), "");
    assert!(!doc.can_undo());
}

#[test]
fn typing_merges_into_one_step() {
    let (_tmp, mut doc) = new_doc();
    for (i, c) in "hello".chars().enumerate() {
        doc.insert(i, &c.to_string()).unwrap();
    }
    doc.delete(4, 1).unwrap();
    assert!(doc.undo().unwrap().is_some());
    assert_eq!(doc.text(), "");
    assert!(!doc.can_undo());
}

#[test]
fn new_edit_clears_redo() {
    let (_tmp, mut doc) = new_doc();
    doc.insert(0, "hello").unwrap();
    doc.undo().unwrap();
    assert!(doc.can_redo());
    doc.insert(0, "x").unwrap();
    assert!(!doc.can_redo());
    // Typing right after an undo is its own step.
    doc.insert(1, "y").unwrap();
    doc.undo().unwrap();
    assert_eq!(doc.text(), "");
}

#[test]
fn autosave_and_reopen() {
    let (tmp, mut doc) = new_doc();
    doc.insert(0, "Kapitel 1\n").unwrap();
    doc.autosave().unwrap();
    doc.insert(10, "= Einleitung 📖").unwrap();
    doc.delete(0, 2).unwrap();
    doc.autosave().unwrap();
    let expected = doc.text();
    drop(doc);
    let doc = open(tmp.path()).unwrap();
    assert_eq!(doc.text(), expected);
}

#[test]
fn drop_autosaves() {
    let (tmp, mut doc) = new_doc();
    doc.insert(0, "unsaved").unwrap();
    drop(doc);
    assert_eq!(open(tmp.path()).unwrap().text(), "unsaved");
}

#[test]
fn noop_autosave_writes_nothing() {
    let (tmp, mut doc) = new_doc();
    doc.autosave().unwrap();
    assert_eq!(log_len(tmp.path()), 0);
    doc.insert(0, "x").unwrap();
    doc.autosave().unwrap();
    let len = log_len(tmp.path());
    assert!(len > 0);
    doc.autosave().unwrap();
    doc.autosave().unwrap();
    assert_eq!(log_len(tmp.path()), len);
}

#[test]
fn torn_tail_garbage_is_repaired() {
    let (tmp, mut doc) = new_doc();
    doc.insert(0, "a").unwrap();
    doc.autosave().unwrap();
    doc.insert(1, "b").unwrap();
    doc.autosave().unwrap();
    drop(doc);
    let good = log_len(tmp.path());
    let mut log = OpenOptions::new()
        .append(true)
        .open(log_path(tmp.path()))
        .unwrap();
    log.write_all(&[7, 0, 0, 0, 1, 2, 3]).unwrap();
    drop(log);

    let doc = open(tmp.path()).unwrap();
    assert_eq!(doc.text(), "ab");
    assert_eq!(log_len(tmp.path()), good);
}

#[test]
fn truncated_last_record_is_dropped() {
    let (tmp, mut doc) = new_doc();
    doc.insert(0, "a").unwrap();
    doc.autosave().unwrap();
    let first = log_len(tmp.path());
    doc.insert(1, "b").unwrap();
    doc.autosave().unwrap();
    drop(doc);
    let full = log_len(tmp.path());
    let log = OpenOptions::new()
        .write(true)
        .open(log_path(tmp.path()))
        .unwrap();
    log.set_len(full - 3).unwrap();
    drop(log);

    let mut doc = open(tmp.path()).unwrap();
    assert_eq!(doc.text(), "a");
    assert_eq!(log_len(tmp.path()), first);
    // The repaired log keeps working.
    doc.insert(1, "c").unwrap();
    drop(doc);
    assert_eq!(open(tmp.path()).unwrap().text(), "ac");
}

#[test]
fn crc_mismatch_ends_replay() {
    let (tmp, mut doc) = new_doc();
    doc.insert(0, "a").unwrap();
    doc.autosave().unwrap();
    let first = log_len(tmp.path());
    doc.insert(1, "b").unwrap();
    drop(doc);
    let path = log_path(tmp.path());
    let mut bytes = fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    fs::write(&path, bytes).unwrap();

    let doc = open(tmp.path()).unwrap();
    assert_eq!(doc.text(), "a");
    assert_eq!(log_len(tmp.path()), first);
}

/// `<dir>/<device>.log` of `me()`, project-relative.
fn rel_log(dir: &str) -> String {
    format!("{dir}/{}.log", me().device.as_str())
}

#[test]
fn damaged_record_fails_the_open_until_recovered() {
    let (tmp, mut doc) = new_doc();
    let dir = tmp.path();
    doc.insert(0, "a").unwrap();
    doc.autosave().unwrap();
    doc.insert(1, "b").unwrap();
    doc.autosave().unwrap();
    let good = log_len(dir) as usize;
    doc.insert(2, "c").unwrap();
    drop(doc);
    // A record Loro rejects, followed by a good one.
    let path = log_path(dir);
    let old = fs::read(&path).unwrap();
    let mut bytes = old[..good].to_vec();
    bytes.extend_from_slice(&frame(b"not a loro update"));
    bytes.extend_from_slice(&old[good..]);
    fs::write(&path, &bytes).unwrap();

    match open(dir) {
        Err(Error::Damaged { file }) => assert_eq!(file, rel_log("main")),
        other => panic!("{:?}", other.map(|d| d.text())),
    }
    assert_eq!(fs::read(&path).unwrap(), bytes, "the log is unchanged");
    assert!(!dir.join("backup").exists());

    let mut doc = Document::open_at_last_good(dir, &me()).unwrap();
    assert_eq!(
        doc.text(),
        "ab",
        "the damaged record and all after it dropped"
    );
    let backup = format!("backup/damaged/main-{}.log", me().device.as_str());
    assert_eq!(
        doc.recovered(),
        [Recovered {
            file: rel_log("main"),
            dropped: (bytes.len() - good) as u64,
            backup: backup.clone(),
        }]
    );
    assert_eq!(
        fs::read(dir.join(&backup)).unwrap(),
        bytes,
        "byte-exact copy"
    );
    assert_eq!(log_len(dir), good as u64);
    // The recovered log keeps working.
    doc.insert(2, "x").unwrap();
    drop(doc);
    let doc = open(dir).unwrap();
    assert_eq!(doc.text(), "abx");
    assert!(doc.recovered().is_empty());
    drop(doc);

    // A second copy is numbered; changes whose history is missing count as
    // damaged too.
    let x = LoroDoc::new();
    x.get_text("content").insert(0, "lost ").unwrap();
    x.commit();
    let vv = x.oplog_vv();
    x.get_text("content").insert(5, "pending").unwrap();
    x.commit();
    let pending = frame(&x.export(ExportMode::updates(&vv)).unwrap());
    OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&pending)
        .unwrap();
    assert!(matches!(open(dir), Err(Error::Damaged { .. })));
    let doc = Document::open_at_last_good(dir, &me()).unwrap();
    assert_eq!(doc.text(), "abx");
    assert_eq!(
        doc.recovered()[0].backup,
        format!("backup/damaged/main-{}-2.log", me().device.as_str())
    );
    assert_eq!(doc.recovered()[0].dropped, pending.len() as u64);
}

#[test]
fn damaged_review_record_is_recovered_alike() {
    let (tmp, mut doc) = new_doc();
    let dir = tmp.path();
    doc.insert(0, "Some text").unwrap();
    doc.add_comment(0, 4, "kept").unwrap();
    doc.autosave().unwrap();
    let items = doc.review_items();
    drop(doc);
    let path = dir.join(rel_log("review"));
    let good = fs::metadata(&path).unwrap().len();
    OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&frame(b"not a loro update"))
        .unwrap();
    let bytes = fs::read(&path).unwrap();

    match open(dir) {
        Err(Error::Damaged { file }) => assert_eq!(file, rel_log("review")),
        other => panic!("{:?}", other.map(|d| d.text())),
    }
    assert_eq!(fs::read(&path).unwrap(), bytes);

    let mut doc = Document::open_at_last_good(dir, &me()).unwrap();
    assert_eq!(doc.review_items(), items);
    assert_eq!(doc.text(), "Some text");
    let recovered = doc.recovered().to_vec();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].file, rel_log("review"));
    assert_eq!(fs::read(dir.join(&recovered[0].backup)).unwrap(), bytes);
    assert_eq!(fs::metadata(&path).unwrap().len(), good);
    doc.add_comment(5, 9, "later").unwrap();
    drop(doc);
    assert_eq!(open(dir).unwrap().review_items().len(), 2);
}

#[test]
fn rejected_own_snapshot_names_it_and_changes_nothing() {
    let (tmp, mut doc) = new_doc();
    let dir = tmp.path();
    doc.insert(0, "a").unwrap();
    doc.add_comment(0, 1, "c").unwrap();
    drop(doc);
    // A torn tail in the main log stays: the review snapshot fails first.
    OpenOptions::new()
        .append(true)
        .open(log_path(dir))
        .unwrap()
        .write_all(&[7, 0, 0, 0, 1, 2, 3])
        .unwrap();
    let review_snapshot = format!("review/{}.snapshot", me().device.as_str());
    fs::write(dir.join(&review_snapshot), b"garbage").unwrap();
    let main_snapshot = format!("main/{}.snapshot", me().device.as_str());
    let files = [
        log_path(dir),
        dir.join(&main_snapshot),
        dir.join(&review_snapshot),
    ];
    let before: Vec<Vec<u8>> = files.iter().map(|f| fs::read(f).unwrap()).collect();
    let after = || -> Vec<Vec<u8>> { files.iter().map(|f| fs::read(f).unwrap()).collect() };

    for result in [open(dir), Document::open_at_last_good(dir, &me())] {
        match result {
            Err(Error::Corrupt(why)) => assert!(why.contains(&review_snapshot), "{why}"),
            other => panic!("{:?}", other.map(|d| d.text())),
        }
        assert_eq!(after(), before);
    }
    fs::write(dir.join(&main_snapshot), b"garbage").unwrap();
    match open(dir) {
        Err(Error::Corrupt(why)) => assert!(why.contains(&main_snapshot), "{why}"),
        other => panic!("{:?}", other.map(|d| d.text())),
    }
    assert!(!dir.join("backup").exists());
}

#[test]
fn other_devices_rejected_data_is_listed_as_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let (a, b) = (who(1), who(2));
    let mut doc_a = Document::create(dir, &a).unwrap();
    doc_a.insert(0, "a").unwrap();
    drop(doc_a);
    // A torn tail of A: not listed (it may still be arriving).
    OpenOptions::new()
        .append(true)
        .open(own(dir, &a, "log"))
        .unwrap()
        .write_all(&[7, 0, 0, 0, 1, 2, 3])
        .unwrap();
    // Device 4: a log record Loro rejects, in main and review.
    fs::write(own(dir, &who(4), "log"), frame(b"not a loro update")).unwrap();
    fs::create_dir_all(dir.join("review")).unwrap();
    fs::write(
        dir.join("review")
            .join(format!("{}.snapshot", who(4).device.as_str())),
        b"garbage",
    )
    .unwrap();

    let doc_b = Document::open(dir, &b).unwrap();
    assert_eq!(doc_b.text(), "a");
    let skipped = doc_b.skipped_on_open();
    assert_eq!(skipped.len(), 2, "{skipped:?}");
    let four = who(4).device;
    assert!(skipped[0].starts_with("main: ") && skipped[0].contains(four.as_str()));
    assert!(skipped[1].starts_with("review: ") && skipped[1].contains(four.as_str()));
    assert!(doc_b.recovered().is_empty());
}

#[test]
fn compaction() {
    let (tmp, mut doc) = new_doc();
    doc.set_compaction_threshold(1024);
    let big = "lorem ipsum ".repeat(200);
    doc.insert(0, &big).unwrap();
    doc.autosave().unwrap();
    assert_eq!(log_len(tmp.path()), 0, "large record triggers compaction");
    doc.insert(0, "Titel\n").unwrap();
    doc.autosave().unwrap();
    let small = log_len(tmp.path());
    assert!(small > 0 && small < 1024);
    let expected = doc.text();
    drop(doc);
    assert!(!own(tmp.path(), &me(), "snapshot.tmp").exists());
    let doc = open(tmp.path()).unwrap();
    assert_eq!(doc.text(), expected);
}

#[test]
fn checkpoints_policy_preview_restore() {
    let (tmp, mut doc) = new_doc();
    assert_eq!(doc.maybe_auto_checkpoint(at(0)).unwrap(), None, "empty doc");
    doc.insert(0, "v1").unwrap();
    let named = doc.checkpoint(Some("Draft"), at(0)).unwrap();

    doc.insert(2, " v2").unwrap();
    assert_eq!(
        doc.maybe_auto_checkpoint(at(5 * 60)).unwrap(),
        None,
        "too early"
    );
    let auto = doc.maybe_auto_checkpoint(at(10 * 60)).unwrap().unwrap();
    assert_eq!(
        doc.maybe_auto_checkpoint(at(60 * 60)).unwrap(),
        None,
        "unchanged"
    );

    let cps = doc.checkpoints();
    assert_eq!(cps.len(), 2);
    assert_eq!(
        (cps[0].name.as_deref(), cps[0].auto),
        (Some("Draft"), false)
    );
    assert_eq!((cps[1].name.as_deref(), cps[1].auto), (None, true));
    assert_eq!(cps[1].created - cps[0].created, 600);

    assert_eq!(doc.text_at(named).unwrap(), "v1");
    assert_eq!(doc.text(), "v1 v2");

    doc.insert(5, " v3").unwrap();
    // Restored right after typing: still a step of its own.
    assert_eq!(doc.restore(named).unwrap(), [ch(2, 8, "")]);
    assert_eq!(doc.text(), "v1");
    assert_eq!(doc.text_at(auto).unwrap(), "v1 v2", "history kept");
    doc.undo().unwrap();
    assert_eq!(doc.text(), "v1 v2 v3");
    doc.redo().unwrap();
    assert_eq!(doc.text(), "v1");
    // Typing right after a restore is a step of its own.
    doc.insert(2, "!").unwrap();
    doc.undo().unwrap();
    assert_eq!(doc.text(), "v1");

    assert!(matches!(
        doc.restore(lethora_core::CheckpointId(99)),
        Err(Error::NotFound(_))
    ));

    let listed = doc.checkpoints().to_vec();
    drop(doc);
    let mut doc = open(tmp.path()).unwrap();
    assert_eq!(doc.checkpoints(), &listed[..]);
    assert_eq!(doc.text(), "v1");
    assert_eq!(doc.text_at(auto).unwrap(), "v1 v2");
    doc.restore(auto).unwrap();
    assert_eq!(doc.text(), "v1 v2");
}

#[test]
fn auto_checkpoint_interval_is_configurable() {
    let (_tmp, mut doc) = new_doc();
    doc.set_auto_checkpoint_interval(Duration::from_secs(60));
    doc.insert(0, "a").unwrap();
    assert!(doc.maybe_auto_checkpoint(at(0)).unwrap().is_some(), "first");
    doc.insert(1, "b").unwrap();
    assert!(doc.maybe_auto_checkpoint(at(59)).unwrap().is_none());
    assert!(doc.maybe_auto_checkpoint(at(60)).unwrap().is_some());
}

#[test]
fn apply_changes_uses_codemirror_semantics() {
    let (_tmp, mut doc) = new_doc();
    doc.apply_changes(&[ch(0, 0, "hello world")]).unwrap();
    // All offsets refer to the text before the change set.
    doc.apply_changes(&[ch(0, 5, "Hi"), ch(5, 5, ","), ch(6, 11, "there")])
        .unwrap();
    assert_eq!(doc.text(), "Hi, there");
    // Inserts at the same position keep their order.
    doc.apply_changes(&[ch(9, 9, "a"), ch(9, 9, "b")]).unwrap();
    assert_eq!(doc.text(), "Hi, thereab");

    // Emoji are two UTF-16 units, CJK one.
    doc.apply_changes(&[ch(0, 11, "a🌍b中文")]).unwrap();
    assert_eq!((doc.len(), doc.len_utf16()), (5, 6));
    doc.apply_changes(&[ch(1, 3, "🎉"), ch(4, 5, "字"), ch(6, 6, "𝄞")])
        .unwrap();
    assert_eq!(doc.text(), "a🎉b字文𝄞");
    assert_eq!(doc.len_utf16(), 8);
}

#[test]
fn apply_changes_is_one_undo_step() {
    let (_tmp, mut doc) = new_doc();
    doc.apply_changes(&[]).unwrap();
    doc.apply_changes(&[ch(0, 0, "")]).unwrap();
    assert!(!doc.can_undo(), "no-op changes commit nothing");
    doc.apply_changes(&[ch(0, 0, "hello world")]).unwrap();
    doc.end_undo_step();
    doc.apply_changes(&[ch(0, 0, "🎉"), ch(11, 11, "!")])
        .unwrap();
    doc.apply_changes(&[ch(8, 8, "中")]).unwrap(); // merges with the step above
    assert_eq!(doc.text(), "🎉hello 中world!");
    assert_eq!(
        doc.undo().unwrap().unwrap(),
        [ch(0, 2, ""), ch(8, 9, ""), ch(14, 15, "")]
    );
    assert_eq!(doc.text(), "hello world");
    assert_eq!(
        doc.redo().unwrap().unwrap(),
        [ch(0, 0, "🎉"), ch(6, 6, "中"), ch(11, 11, "!")]
    );
    assert_eq!(doc.text(), "🎉hello 中world!");
    // Replacements come back as one change.
    doc.end_undo_step();
    doc.apply_changes(&[ch(2, 7, "Hallo")]).unwrap();
    assert_eq!(doc.undo().unwrap().unwrap(), [ch(2, 7, "hello")]);
}

#[test]
fn invalid_changes_leave_text_untouched() {
    let (_tmp, mut doc) = new_doc();
    doc.apply_changes(&[ch(0, 0, "a🌍b")]).unwrap(); // UTF-16 length 4
    for bad in [
        vec![ch(3, 3, "x"), ch(0, 0, "y")],    // unsorted
        vec![ch(0, 2, ""), ch(1, 3, "")],      // overlapping
        vec![ch(4, 5, "")],                    // out of range
        vec![ch(5, 5, "x")],                   // out of range
        vec![ch(3, 1, "")],                    // to < from
        vec![ch(2, 2, "x")],                   // inside the emoji
        vec![ch(0, 2, "")],                    // ends inside the emoji
        vec![ch(0, 0, "ok"), ch(2, 3, "x")],   // valid first change, invalid second
        vec![ch(usize::MAX, usize::MAX, "x")], // overflow-sized
    ] {
        assert!(
            matches!(doc.apply_changes(&bad), Err(Error::InvalidChange(_))),
            "{bad:?}"
        );
        assert_eq!(doc.text(), "a🌍b");
    }
    doc.undo().unwrap();
    assert!(doc.is_empty() && !doc.can_undo());
}

/// Random change sets, undo, redo and restore; after every operation the
/// returned changes, applied CodeMirror-style to a shadow copy, must
/// reproduce the document text.
#[test]
fn changes_round_trip() {
    const PIECES: [&str; 10] = ["a", "b", " ", "\n", "é", "中", "文", "🌍", "𝄞", "ß"];
    let (_tmp, mut doc) = new_doc();
    let mut next = prng(0x9e37_79b9_7f4a_7c15);
    let mut shadow = String::new();
    let mut checkpoints: Vec<CheckpointId> = Vec::new();
    for i in 0..400 {
        let op = next(100);
        let changes = if op < 60 {
            // A change set at random char boundaries, in UTF-16 offsets.
            let chars: Vec<char> = shadow.chars().collect();
            let utf16 = |n: usize| chars[..n].iter().map(|c| c.len_utf16()).sum::<usize>();
            let mut cuts: Vec<usize> = (0..2 * next(4)).map(|_| next(chars.len() + 1)).collect();
            cuts.sort();
            let changes: Vec<TextChange> = cuts
                .chunks(2)
                .map(|p| {
                    let insert: String = (0..next(5)).map(|_| PIECES[next(PIECES.len())]).collect();
                    ch(utf16(p[0]), utf16(p[1]), &insert)
                })
                .collect();
            doc.apply_changes(&changes).unwrap();
            if next(4) == 0 {
                doc.end_undo_step();
            }
            Some(changes)
        } else if op < 78 {
            doc.undo().unwrap()
        } else if op < 92 {
            doc.redo().unwrap()
        } else if op < 96 || checkpoints.is_empty() {
            checkpoints.push(doc.checkpoint(None, at(i)).unwrap());
            None
        } else {
            let id = checkpoints[next(checkpoints.len())];
            let changes = doc.restore(id).unwrap();
            assert_eq!(doc.text(), doc.text_at(id).unwrap());
            Some(changes)
        };
        if let Some(changes) = changes {
            shadow = apply_cm(&shadow, &changes);
        }
        assert_eq!(doc.text(), shadow, "op {i}");
        assert_eq!(doc.len_utf16(), shadow.encode_utf16().count());
    }
}

#[test]
fn project_lock() {
    let tmp = tempfile::tempdir().unwrap();
    let mut doc = create(tmp.path()).unwrap();
    assert!(matches!(open(tmp.path()), Err(Error::Locked(_))));
    doc.insert(0, "a").unwrap();
    doc.autosave().unwrap();
    // A torn tail is not repaired while another instance holds the project.
    let good = log_len(tmp.path());
    let mut log = OpenOptions::new()
        .append(true)
        .open(log_path(tmp.path()))
        .unwrap();
    log.write_all(&[7, 0, 0]).unwrap();
    drop(log);
    assert!(matches!(open(tmp.path()), Err(Error::Locked(_))));
    assert_eq!(log_len(tmp.path()), good + 3);

    drop(doc);
    let doc = open(tmp.path()).unwrap();
    assert_eq!(log_len(tmp.path()), good);
    assert!(matches!(open(tmp.path()), Err(Error::Locked(_))));
    drop(doc);
    assert_eq!(open(tmp.path()).unwrap().text(), "a");
}

/// Going back across one big paste must stay fast. Unpatched loro 1.16.2 is
/// quadratic in the paste length here (~1.5 s at 400k chars in release, minutes
/// at book size); see vendor/loro-internal-fix.patch.
#[test]
fn back_across_big_paste_is_fast() {
    let (_tmp, mut doc) = new_doc();
    doc.insert(0, "A short beginning.").unwrap();
    let small = doc.checkpoint(Some("small"), at(0)).unwrap();
    doc.end_undo_step();
    let paste = "lorem ipsum dolor sit amet consectetur adipiscing elit ".repeat(400_000 / 55);
    doc.insert(doc.len(), &paste).unwrap();
    let limit = Duration::from_secs(2);

    let t = Instant::now();
    assert_eq!(doc.text_at(small).unwrap(), "A short beginning.");
    assert!(t.elapsed() < limit, "text_at took {:?}", t.elapsed());

    let t = Instant::now();
    assert!(doc.undo().unwrap().is_some());
    assert!(t.elapsed() < limit, "undo took {:?}", t.elapsed());
    assert_eq!(doc.text(), "A short beginning.");

    assert!(doc.redo().unwrap().is_some());
    let t = Instant::now();
    doc.restore(small).unwrap();
    assert!(t.elapsed() < limit, "restore took {:?}", t.elapsed());
    assert_eq!(doc.text(), "A short beginning.");
}

/// Book-size scenario (~1M words). Run with:
/// `cargo test --release -p lethora-core -- --ignored --nocapture`
#[test]
#[ignore]
fn book_size() {
    let mut rng = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move |n: usize| {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng % n as u64) as usize
    };
    const WORDS: [&str; 8] = [
        "the",
        "quantum",
        "Ölberg",
        "manuscript",
        "über",
        "réseau",
        "chapter",
        "ink",
    ];

    let (tmp, mut doc) = new_doc();
    let t = Instant::now();
    for _ in 0..100 {
        let mut chunk = String::new();
        for _ in 0..10_000 {
            chunk.push_str(WORDS[next(WORDS.len())]);
            chunk.push(' ');
        }
        let pos = next(doc.len() + 1);
        doc.insert(pos, &chunk).unwrap();
    }
    let bulk = t.elapsed();
    let chars = doc.len();

    let t = Instant::now();
    for _ in 0..10_000 {
        if next(3) == 0 {
            let pos = next(doc.len() - 5);
            doc.delete(pos, 1 + next(5)).unwrap();
        } else {
            let pos = next(doc.len() + 1);
            doc.insert(pos, WORDS[next(WORDS.len())]).unwrap();
        }
    }
    let edits = t.elapsed();

    let t = Instant::now();
    doc.autosave().unwrap();
    let first_save = t.elapsed();

    doc.insert(1000, "x").unwrap();
    let t = Instant::now();
    doc.autosave().unwrap();
    let small_save = t.elapsed();

    let expected = doc.text();
    drop(doc);
    let t = Instant::now();
    let mut doc = open(tmp.path()).unwrap();
    let open = t.elapsed();
    assert_eq!(doc.text(), expected);
    let t = Instant::now();
    doc.insert(500, "y").unwrap();
    let first_edit = t.elapsed();

    // Undo and restore with editor changes: 1000 scattered edits, one step.
    let cp = doc.checkpoint(Some("book"), at(0)).unwrap();
    doc.end_undo_step();
    for _ in 0..1000 {
        if next(3) == 0 {
            let pos = next(doc.len() - 5);
            doc.delete(pos, 1 + next(5)).unwrap();
        } else {
            let pos = next(doc.len() + 1);
            doc.insert(pos, "🌍 中").unwrap();
        }
    }
    let before = doc.text();
    let t = Instant::now();
    let undone = doc.undo().unwrap().unwrap();
    let undo = t.elapsed();
    assert_eq!(apply_cm(&before, &undone), doc.text());
    doc.redo().unwrap();
    let before = doc.text();
    let t = Instant::now();
    let restored = doc.restore(cp).unwrap();
    let restore = t.elapsed();
    assert_eq!(apply_cm(&before, &restored), doc.text());
    let t = Instant::now();
    let _ = doc.text();
    let text_copy = t.elapsed();
    // "Who wrote this" for a generous viewport (20k UTF-16 units).
    let mid = doc.len_utf16() / 2;
    let t = Instant::now();
    let runs = doc.authors_in(mid, mid + 20_000);
    let authors = t.elapsed();

    let size = |ext: &str| fs::metadata(own(tmp.path(), &me(), ext)).unwrap().len();
    println!(
        "book_size: {chars} chars; 100 bulk inserts {bulk:?}; 10k edits {edits:?} \
         ({:.1} us/edit); first autosave {first_save:?}; small autosave {small_save:?}; \
         open {open:?}; first edit after open {first_edit:?}; snapshot {} B, log {} B; \
         undo {undo:?} ({} changes); restore {restore:?} ({} changes); text() {text_copy:?}; \
         authors of 20k units {authors:?} ({} runs)",
        edits.as_secs_f64() * 1e6 / 10_000.0,
        size("snapshot"),
        size("log"),
        undone.len(),
        restored.len(),
        runs.len(),
    );
}

// ---- Settings: layout/meta maps and custom code ----

fn changes(pairs: &[(&str, Option<Value>)]) -> BTreeMap<String, Option<Value>> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect()
}

#[test]
fn map_update_is_one_undo_step() {
    let (_tmp, mut doc) = new_doc();
    assert!(doc.map_entries(MapName::Layout).is_empty());
    doc.map_update(
        MapName::Layout,
        &changes(&[
            ("preset", Some(json!("novel"))),
            ("page.columns", Some(json!(2))),
            ("text.size", Some(json!("11pt"))),
            ("text.justify", Some(json!(true))),
            ("figures.default_width", Some(json!(80.5))),
            ("x", Some(Value::Null)),
        ]),
    )
    .unwrap();
    let first = doc.map_entries(MapName::Layout);
    assert_eq!(first.len(), 6);
    assert_eq!(first["page.columns"], json!(2));
    assert_eq!(first["figures.default_width"], json!(80.5));
    assert_eq!(first["x"], Value::Null);
    assert!(
        doc.map_entries(MapName::Meta).is_empty(),
        "maps are separate"
    );

    doc.end_undo_step();
    doc.map_update(
        MapName::Layout,
        &changes(&[("text.size", Some(json!("12pt"))), ("x", None)]),
    )
    .unwrap();
    let second = doc.map_entries(MapName::Layout);
    assert_eq!(second["text.size"], json!("12pt"));
    assert!(!second.contains_key("x"));
    // Settings-only steps undo with an empty (but present) text change list.
    assert_eq!(doc.undo().unwrap(), Some(vec![]));
    assert_eq!(doc.map_entries(MapName::Layout), first);
    assert_eq!(doc.redo().unwrap(), Some(vec![]));
    assert_eq!(doc.map_entries(MapName::Layout), second);
    assert_eq!(doc.undo().unwrap(), Some(vec![]));
    assert_eq!(doc.undo().unwrap(), Some(vec![]));
    assert!(doc.map_entries(MapName::Layout).is_empty());
    assert!(!doc.can_undo());
}

#[test]
fn map_update_rejects_bad_input_and_skips_noops() {
    let (_tmp, mut doc) = new_doc();
    let long = "k".repeat(300);
    for bad in [
        changes(&[("ok", Some(json!(1))), ("", Some(json!(1)))]),
        changes(&[("ok", Some(json!(1))), (long.as_str(), Some(json!(1)))]),
        changes(&[("ok", Some(json!(1))), ("list", Some(json!([1, 2])))]),
        changes(&[("ok", Some(json!(1))), ("obj", Some(json!({"a": 1})))]),
    ] {
        assert!(matches!(
            doc.map_update(MapName::Meta, &bad),
            Err(Error::InvalidChange(_))
        ));
        assert!(doc.map_entries(MapName::Meta).is_empty());
    }
    assert!(!doc.can_undo());
    doc.map_update(MapName::Meta, &changes(&[("missing", None)]))
        .unwrap();
    assert!(!doc.can_undo(), "deleting a missing key is a no-op");
    doc.map_update(MapName::Meta, &changes(&[("title", Some(json!("T")))]))
        .unwrap();
    doc.end_undo_step();
    doc.map_update(MapName::Meta, &changes(&[("title", Some(json!("T")))]))
        .unwrap();
    doc.undo().unwrap();
    assert!(
        doc.map_entries(MapName::Meta).is_empty(),
        "same value: no step"
    );
}

#[test]
fn map_replace_switches_presets_in_one_step() {
    let (_tmp, mut doc) = new_doc();
    doc.map_update(
        MapName::Layout,
        &changes(&[
            ("preset", Some(json!("paper"))),
            ("text.size", Some(json!("10pt"))),
        ]),
    )
    .unwrap();
    doc.end_undo_step();
    let preset: Entries = [("preset".to_owned(), json!("novel"))].into();
    doc.map_replace(MapName::Layout, &preset).unwrap();
    assert_eq!(doc.map_entries(MapName::Layout), preset);
    doc.undo().unwrap();
    assert_eq!(doc.map_entries(MapName::Layout).len(), 2);
    assert_eq!(doc.map_entries(MapName::Layout)["preset"], json!("paper"));
}

#[test]
fn settings_and_typing_are_separate_steps() {
    let (_tmp, mut doc) = new_doc();
    doc.insert(0, "Hello").unwrap();
    // Within the merge interval, but a different part: a new step.
    doc.map_update(MapName::Layout, &changes(&[("a", Some(json!(1)))]))
        .unwrap();
    doc.map_update(MapName::Layout, &changes(&[("b", Some(json!(2)))]))
        .unwrap();
    doc.set_custom_text("#set page(fill: red)").unwrap();
    doc.insert(5, "!").unwrap();

    assert_eq!(doc.undo().unwrap().unwrap(), [ch(5, 6, "")]);
    assert_eq!(doc.map_entries(MapName::Layout).len(), 2);
    // Consecutive settings edits merge like typing.
    assert_eq!(doc.undo().unwrap(), Some(vec![]));
    assert!(doc.map_entries(MapName::Layout).is_empty());
    assert_eq!(doc.custom_text(), "");
    assert_eq!(doc.text(), "Hello");
    assert_eq!(doc.undo().unwrap().unwrap(), [ch(0, 5, "")]);
    assert!(!doc.can_undo());
}

#[test]
fn mixed_steps_restore_and_reopen() {
    let (tmp, mut doc) = new_doc();
    doc.insert(0, "Text").unwrap();
    doc.map_update(
        MapName::Layout,
        &changes(&[("preset", Some(json!("paper")))]),
    )
    .unwrap();
    doc.map_update(
        MapName::Meta,
        &changes(&[("title", Some(json!("Über \"x\"")))]),
    )
    .unwrap();
    doc.set_custom_text("#let a = 1").unwrap();
    let cp = doc.checkpoint(Some("v1"), at(0)).unwrap();

    doc.end_undo_step();
    doc.insert(4, " more").unwrap();
    doc.map_update(
        MapName::Layout,
        &changes(&[
            ("preset", Some(json!("novel"))),
            ("text.size", Some(json!("9pt"))),
        ]),
    )
    .unwrap();
    doc.set_custom_text("#let a = 2\n#let b = 3").unwrap();

    // Restore covers text and settings; only the text changes are reported.
    assert_eq!(doc.restore(cp).unwrap(), [ch(4, 9, "")]);
    assert_eq!(doc.text(), "Text");
    let restored: Entries = [("preset".to_owned(), json!("paper"))].into();
    assert_eq!(doc.map_entries(MapName::Layout), restored);
    assert_eq!(doc.custom_text(), "#let a = 1");
    // Undoing the restore brings everything back, reporting the text.
    assert_eq!(doc.undo().unwrap().unwrap(), [ch(4, 4, " more")]);
    assert_eq!(doc.map_entries(MapName::Layout)["preset"], json!("novel"));
    assert_eq!(doc.custom_text(), "#let a = 2\n#let b = 3");

    let layout = doc.map_entries(MapName::Layout);
    let meta = doc.map_entries(MapName::Meta);
    drop(doc);
    let mut doc = open(tmp.path()).unwrap();
    assert_eq!(doc.map_entries(MapName::Layout), layout);
    assert_eq!(doc.map_entries(MapName::Meta), meta);
    assert_eq!(doc.map_entries(MapName::Meta)["title"], json!("Über \"x\""));
    assert_eq!(doc.custom_text(), "#let a = 2\n#let b = 3");
    doc.restore(cp).unwrap();
    assert_eq!(doc.custom_text(), "#let a = 1");
}

#[test]
fn settings_survive_a_crash_after_autosave() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("p");
    let copy = tmp.path().join("copy");
    {
        let mut doc = create(&dir).unwrap();
        doc.map_update(MapName::Layout, &changes(&[("k", Some(json!("v")))]))
            .unwrap();
        doc.set_custom_text("abc").unwrap();
        doc.autosave().unwrap();
        // A crash right now: copy the files while the document is open.
        let id = me().device;
        let id = id.as_str();
        for name in [
            "project.json".to_owned(),
            format!("main/{id}.snapshot"),
            format!("main/{id}.log"),
            format!("checkpoints/{id}.json"),
        ] {
            fs::create_dir_all(copy.join(&name).parent().unwrap()).unwrap();
            fs::copy(dir.join(&name), copy.join(&name)).unwrap();
        }
    }
    let doc = open(&copy).unwrap();
    assert_eq!(doc.map_entries(MapName::Layout)["k"], json!("v"));
    assert_eq!(doc.custom_text(), "abc");
}

#[test]
fn custom_text_replaces_minimally() {
    let (_tmp, mut doc) = new_doc();
    doc.set_custom_text("").unwrap();
    assert!(!doc.can_undo());
    for text in [
        "#set text(size: 11pt)",
        "#set text(size: 12pt)",
        "#set text(size: 12pt)\n#set par(justify: true)",
        "🌍 #set text(size: 12pt)\n#set par(justify: true)",
        "🌍🌍",
        "aaa",
        "aa",
        "aaaa",
        "",
    ] {
        doc.end_undo_step();
        doc.set_custom_text(text).unwrap();
        assert_eq!(doc.custom_text(), text);
    }
    doc.undo().unwrap();
    assert_eq!(doc.custom_text(), "aaaa");
    assert_eq!(doc.text(), "", "content untouched");
}

// ---- Anchors and the overrides map ----

/// UTF-16 offset of the first `needle` in `text`.
fn utf16_of(text: &str, needle: &str) -> usize {
    text[..text.find(needle).unwrap()].encode_utf16().count()
}

#[test]
fn anchors_follow_their_character() {
    let (tmp, mut doc) = new_doc();
    doc.apply_changes(&[ch(0, 0, "Intro 🌍\n\n= Chapter\n\nText.")])
        .unwrap();
    let at = utf16_of(&doc.text(), "= Chapter");
    let anchor = doc.anchor_at(at).unwrap();
    let pos = |doc: &Document| doc.resolve_anchor(&anchor).unwrap();
    assert_eq!(pos(&doc).pos_utf16, at);
    assert_eq!(pos(&doc).pos_byte, doc.text().find("= Chapter").unwrap());
    assert!(!pos(&doc).detached);

    // Inserted before, at (still before the character) and after.
    doc.end_undo_step();
    doc.apply_changes(&[ch(0, 0, "New ")]).unwrap();
    assert_eq!(pos(&doc).pos_utf16, at + 4);
    doc.end_undo_step();
    doc.apply_changes(&[ch(at + 4, at + 4, "x\n")]).unwrap();
    assert_eq!(pos(&doc).pos_utf16, at + 6);
    doc.end_undo_step();
    doc.apply_changes(&[ch(at + 7, at + 7, "y")]).unwrap();
    assert_eq!(pos(&doc).pos_utf16, at + 6);
    // Deleted around it.
    doc.end_undo_step();
    doc.apply_changes(&[ch(at + 4, at + 6, ""), ch(at + 7, at + 8, "")])
        .unwrap();
    assert_eq!(pos(&doc).pos_utf16, at + 4);
    assert!(!pos(&doc).detached);
    // Undo/redo of the edits around it.
    doc.undo().unwrap();
    assert_eq!(pos(&doc).pos_utf16, at + 6);
    doc.undo().unwrap();
    doc.undo().unwrap();
    doc.undo().unwrap();
    assert_eq!(pos(&doc).pos_utf16, at);
    doc.redo().unwrap();
    assert_eq!(pos(&doc).pos_utf16, at + 4);
    assert!(!pos(&doc).detached);

    // Survives reopening.
    drop(doc);
    let mut doc = open(tmp.path()).unwrap();
    assert_eq!(pos(&doc).pos_utf16, at + 4);

    // Deleting the anchored character detaches it; it stays where it was.
    let start = utf16_of(&doc.text(), "= Chapter");
    doc.end_undo_step();
    doc.apply_changes(&[ch(start - 2, start + 1, "")]).unwrap();
    let detached = pos(&doc);
    assert!(detached.detached);
    assert_eq!(detached.pos_utf16, start - 2);
    // Undo brings the text back as new characters: the anchor stays
    // detached and resolves inside the restored text (here: on the heading's
    // line, after its marker).
    doc.undo().unwrap();
    let back = pos(&doc);
    assert!(back.detached);
    assert!(
        (start - 2..=start + 1).contains(&back.pos_utf16),
        "{back:?}"
    );
}

#[test]
fn anchors_at_the_ends_and_bad_anchors() {
    let (_tmp, mut doc) = new_doc();
    // In an empty text: stays at the start.
    let empty = doc.anchor_at(0).unwrap();
    doc.insert(0, "abc").unwrap();
    assert_eq!(doc.resolve_anchor(&empty).unwrap().pos_utf16, 0);
    // At the end: stays at the end.
    let end = doc.anchor_at(3).unwrap();
    doc.end_undo_step();
    doc.insert(3, "de").unwrap();
    assert_eq!(doc.resolve_anchor(&end).unwrap().pos_utf16, 5);
    assert!(!doc.resolve_anchor(&end).unwrap().detached);

    assert!(matches!(doc.anchor_at(6), Err(Error::OutOfRange { .. })));
    doc.insert(0, "🌍").unwrap();
    assert!(matches!(doc.anchor_at(1), Err(Error::InvalidChange(_))));
    for bad in ["", "zz", "abc", "00ff", &"ab".repeat(600)] {
        assert!(doc.resolve_anchor(bad).is_err(), "{bad}");
    }
    // An anchor on a character of another document is not found.
    let (_tmp2, mut other) = new_doc();
    other.insert(0, "xyz").unwrap();
    let foreign = other.anchor_at(1).unwrap();
    assert!(doc.resolve_anchor(&foreign).is_err());
}

#[test]
fn overrides_map_round_trips_and_shares_the_history() {
    let (tmp, mut doc) = new_doc();
    doc.insert(0, "= One\n\nText").unwrap();
    let anchor = doc.anchor_at(0).unwrap();
    doc.map_update(
        MapName::Overrides,
        &changes(&[
            ("a1.kind", Some(json!("page_break"))),
            ("a1.anchor", Some(json!(anchor))),
            ("a1.to_odd", Some(json!(true))),
        ]),
    )
    .unwrap();
    assert_eq!(doc.map_entries(MapName::Overrides).len(), 3);
    assert!(doc.map_entries(MapName::Layout).is_empty());
    // An overrides-only step undoes with no text changes.
    assert_eq!(doc.undo().unwrap(), Some(vec![]));
    assert!(doc.map_entries(MapName::Overrides).is_empty());
    assert_eq!(doc.text(), "= One\n\nText");
    assert_eq!(doc.redo().unwrap(), Some(vec![]));
    let entries = doc.map_entries(MapName::Overrides);
    assert_eq!(entries["a1.to_odd"], json!(true));
    drop(doc);
    let doc = open(tmp.path()).unwrap();
    assert_eq!(doc.map_entries(MapName::Overrides), entries);
}

// ---- Devices and the project folder format ----

#[test]
fn device_ids_are_32_lowercase_hex() {
    let id = DeviceId::parse("0123456789abcdef0123456789abcdef").unwrap();
    assert_eq!(id.as_str(), "0123456789abcdef0123456789abcdef");
    for bad in [
        "",
        "0123456789abcdef0123456789abcde",
        "0123456789abcdef0123456789abcdef0",
        "0123456789ABCDEF0123456789abcdef",
        "0123456789abcdef0123456789abcdeg",
        " 123456789abcdef0123456789abcdef",
        "../3456789abcdef0123456789abcdef",
    ] {
        assert!(DeviceId::parse(bad).is_err(), "{bad:?}");
        assert!(UserId::parse(bad).is_err(), "{bad:?}");
    }
    let user = UserId::parse("fedcba9876543210fedcba9876543210").unwrap();
    assert_eq!(user.as_str(), "fedcba9876543210fedcba9876543210");
}

#[test]
fn devices_share_a_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let (a, b) = (who(1), who(2));
    let mut doc_a = Document::create(dir, &a).unwrap();
    doc_a.insert(0, "A wrote this.").unwrap();
    doc_a.autosave().unwrap();

    // Both devices can have it open at once; each device only once.
    let mut doc_b = Document::open(dir, &b).unwrap();
    assert_eq!(doc_b.text(), "A wrote this.");
    assert!(matches!(Document::open(dir, &a), Err(Error::Locked(_))));
    assert!(matches!(Document::open(dir, &b), Err(Error::Locked(_))));
    doc_b.insert(doc_b.len(), " B too.").unwrap();
    doc_a.insert(0, "Title. ").unwrap();
    drop(doc_b);
    drop(doc_a);

    let doc_a = Document::open(dir, &a).unwrap();
    assert_eq!(doc_a.text(), "Title. A wrote this. B too.");
    drop(doc_a);
    let doc_b = Document::open(dir, &b).unwrap();
    assert_eq!(doc_b.text(), "Title. A wrote this. B too.");
    // Each device wrote only its own files; B gets a snapshot when it
    // compacts.
    let mut names: Vec<String> = fs::read_dir(dir.join("main"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    let (a, b) = (a.device.as_str(), b.device.as_str());
    assert_eq!(
        names,
        [
            format!("{a}.log"),
            format!("{a}.snapshot"),
            format!("{b}.log")
        ]
    );
}

#[test]
fn own_files_are_self_contained_after_an_autosave() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let (a, b) = (who(1), who(2));
    let mut doc_a = Document::create(dir, &a).unwrap();
    doc_a.insert(0, "from A").unwrap();
    drop(doc_a);

    let mut doc_b = Document::open(dir, &b).unwrap();
    // Nothing edited on B, yet its log takes in what came from A.
    doc_b.autosave().unwrap();
    assert!(fs::metadata(own(dir, &b, "log")).unwrap().len() > 0);
    doc_b.insert(6, ", then B").unwrap();
    drop(doc_b);

    fs::remove_file(own(dir, &a, "snapshot")).unwrap();
    fs::remove_file(own(dir, &a, "log")).unwrap();
    let doc_b = Document::open(dir, &b).unwrap();
    assert_eq!(doc_b.text(), "from A, then B");
}

#[test]
fn other_devices_files_are_read_only_and_never_fail_the_open() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let (a, b) = (who(1), who(2));
    let mut doc_a = Document::create(dir, &a).unwrap();
    doc_a.insert(0, "a").unwrap();
    doc_a.autosave().unwrap();
    doc_a.insert(1, "b").unwrap();
    drop(doc_a);

    // A torn tail in A's log (e.g. still being synced).
    let a_log = own(dir, &a, "log");
    let mut log = OpenOptions::new().append(true).open(&a_log).unwrap();
    log.write_all(&[7, 0, 0, 0, 1, 2, 3]).unwrap();
    drop(log);
    let a_bytes = fs::read(&a_log).unwrap();
    let a_snapshot = fs::read(own(dir, &a, "snapshot")).unwrap();

    // Device 3: changes whose history never arrived (pending in Loro).
    let x = LoroDoc::new();
    x.get_text("content").insert(0, "lost ").unwrap();
    x.commit();
    let vv = x.oplog_vv();
    x.get_text("content").insert(5, "pending").unwrap();
    x.commit();
    let update = x.export(ExportMode::updates(&vv)).unwrap();
    fs::write(own(dir, &who(3), "log"), frame(&update)).unwrap();
    // Device 4: a garbage snapshot and a log record Loro rejects.
    fs::write(own(dir, &who(4), "snapshot"), b"garbage").unwrap();
    fs::write(own(dir, &who(4), "log"), frame(b"not a loro update")).unwrap();
    // Not device files.
    fs::write(dir.join("main").join("notes.txt"), b"x").unwrap();
    fs::write(dir.join("main").join("XYZ.log"), b"x").unwrap();

    let mut doc_b = Document::open(dir, &b).unwrap();
    assert_eq!(doc_b.text(), "ab");
    doc_b.insert(2, "c").unwrap();
    drop(doc_b);
    assert_eq!(fs::read(&a_log).unwrap(), a_bytes, "A's log untouched");
    assert_eq!(fs::read(own(dir, &a, "snapshot")).unwrap(), a_snapshot);
    assert_eq!(Document::open(dir, &b).unwrap().text(), "abc");
    // A repairs its own torn tail.
    let doc_a = Document::open(dir, &a).unwrap();
    assert_eq!(doc_a.text(), "abc");
    assert!(fs::metadata(&a_log).unwrap().len() < a_bytes.len() as u64);
}

#[test]
fn format1_projects_are_converted() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let old = LoroDoc::new();
    old.get_text("content").insert(0, "Old text").unwrap();
    old.commit();
    let v1 = old.oplog_frontiers();
    let vv = old.oplog_vv();
    fs::write(
        dir.join("snapshot.loro"),
        old.export(ExportMode::all_updates()).unwrap(),
    )
    .unwrap();
    old.get_text("content").insert(8, " and more").unwrap();
    old.commit();
    let mut log = frame(&old.export(ExportMode::updates(&vv)).unwrap());
    log.extend_from_slice(&[9, 0, 0, 0, 1, 2]); // torn tail
    fs::write(dir.join("updates.log"), &log).unwrap();
    let cps = vec![
        Checkpoint {
            id: CheckpointId(1),
            name: Some("Old".into()),
            auto: false,
            created: 1_700_000_000,
            version: v1.encode(),
        },
        Checkpoint {
            id: CheckpointId(2),
            name: None,
            auto: true,
            created: 1_700_000_100,
            version: old.oplog_frontiers().encode(),
        },
    ];
    fs::write(
        dir.join("checkpoints.json"),
        serde_json::to_vec(&cps).unwrap(),
    )
    .unwrap();
    fs::write(dir.join("lock"), b"").unwrap();
    fs::write(dir.join("snapshot.loro.tmp"), b"half").unwrap();

    assert!(matches!(create(dir), Err(Error::AlreadyExists(_))));
    let doc = open(dir).unwrap();
    assert_eq!(doc.text(), "Old text and more");
    assert_eq!(doc.checkpoints(), &cps[..]);
    assert_eq!(doc.text_at(CheckpointId(1)).unwrap(), "Old text");
    assert_eq!(doc.text_at(CheckpointId(2)).unwrap(), "Old text and more");
    // The named one moved into the document, the automatic one stays in the
    // own file.
    assert_eq!(own_checkpoints(dir, &me()), &cps[1..]);
    // The old history (snapshot and log) is attributed to the converting
    // identity; format 1 recorded no times.
    let mine = Some(Authorship {
        user: me().user,
        device: me().device,
        time: None,
    });
    assert_eq!(doc.author_at(0).unwrap(), mine);
    assert_eq!(doc.author_at(10).unwrap(), mine);
    let project: Value =
        serde_json::from_slice(&fs::read(dir.join("project.json")).unwrap()).unwrap();
    assert_eq!(project, json!({"format": 2}));
    let backup = dir.join("format1-backup");
    for name in [
        "snapshot.loro",
        "updates.log",
        "checkpoints.json",
        "lock",
        "snapshot.loro.tmp",
    ] {
        assert!(!dir.join(name).exists(), "{name}");
        assert!(backup.join(name).exists(), "{name}");
    }
    assert_eq!(fs::read(backup.join("updates.log")).unwrap(), log);
    drop(doc);

    let doc = open(dir).unwrap();
    assert_eq!(doc.text(), "Old text and more");
    assert_eq!(doc.checkpoints(), &cps[..]);
    drop(doc);
    // Another device sees the converted text and the named checkpoint (not
    // the automatic one).
    let mut other = Document::open(dir, &who(2)).unwrap();
    assert_eq!(other.text(), "Old text and more");
    assert_eq!(other.checkpoints(), &cps[..1]);
    assert_eq!(other.author_at(0).unwrap(), mine);
    assert!(matches!(
        other.text_at(CheckpointId(2)),
        Err(Error::NotFound(_))
    ));
    other.restore(CheckpointId(1)).unwrap();
    assert_eq!(other.text(), "Old text");
}

#[test]
fn unknown_formats_and_missing_projects() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    assert!(matches!(open(dir), Err(Error::NotFound(_))));
    assert_eq!(fs::read_dir(dir).unwrap().count(), 0, "no lock file left");
    drop(create(dir).unwrap());

    fs::write(dir.join("project.json"), r#"{"format": 3}"#).unwrap();
    match open(dir) {
        Err(Error::UnsupportedFormat(why)) => assert!(why.contains("newer"), "{why}"),
        other => panic!("{:?}", other.map(|d| d.text())),
    }
    for bad in [r#"{"format": 1}"#, "{}", "garbage"] {
        fs::write(dir.join("project.json"), bad).unwrap();
        assert!(
            matches!(open(dir), Err(Error::UnsupportedFormat(_))),
            "{bad}"
        );
    }
    fs::write(dir.join("project.json"), r#"{"format": 2}"#).unwrap();
    assert!(open(dir).unwrap().is_empty());
}

#[test]
fn changes_record_their_time() {
    let (tmp, mut doc) = new_doc();
    doc.insert(0, "x").unwrap();
    doc.autosave().unwrap();
    let log = fs::read(log_path(tmp.path())).unwrap();
    let meta = LoroDoc::decode_import_blob_meta(&log[8..], true).unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert!(
        (now - 60..=now + 60).contains(&meta.start_timestamp),
        "{meta:?}"
    );
}

// ---- Attribution ----

/// Peer id → (user, device) entries of the `peers` map in the project's
/// files, read with Loro directly.
/// Every device's files in `dir`, imported into one Loro document.
fn on_disk(dir: &Path) -> LoroDoc {
    let doc = LoroDoc::new();
    let mut files: Vec<PathBuf> = fs::read_dir(dir.join("main"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    // Snapshots before logs.
    files.sort_by_key(|p| p.extension() != Some("snapshot".as_ref()));
    for path in files {
        let bytes = fs::read(&path).unwrap();
        if path.extension() == Some("snapshot".as_ref()) {
            doc.import(&bytes).unwrap();
            continue;
        }
        let mut at = 0;
        while at < bytes.len() {
            let len = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            doc.import(&bytes[at + 8..at + 8 + len]).unwrap();
            at += 8 + len;
        }
    }
    doc
}

fn peers_on_disk(dir: &Path) -> BTreeMap<String, (String, String)> {
    let doc = on_disk(dir);
    let LoroValue::Map(peers) = doc.get_map("peers").get_value() else {
        unreachable!()
    };
    peers
        .iter()
        .map(|(peer, v)| {
            let LoroValue::Map(v) = v else {
                panic!("{v:?}")
            };
            let field = |k: &str| match &v[k] {
                LoroValue::String(s) => s.to_string(),
                other => panic!("{other:?}"),
            };
            (peer.clone(), (field("user"), field("device")))
        })
        .collect()
}

fn entry(id: &Identity) -> (String, String) {
    (id.user.as_str().to_owned(), id.device.as_str().to_owned())
}

/// `author_at(pos)` as user and device.
fn author(doc: &Document, pos: usize) -> Option<(UserId, DeviceId)> {
    doc.author_at(pos).unwrap().map(|a| (a.user, a.device))
}

fn by(id: &Identity) -> Option<(UserId, DeviceId)> {
    Some((id.user.clone(), id.device.clone()))
}

#[test]
fn peers_register_lazily_once_per_session() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut doc = create(dir).unwrap();
    doc.insert(0, "").unwrap();
    doc.apply_changes(&[ch(0, 0, "")]).unwrap();
    doc.map_update(MapName::Meta, &changes(&[("missing", None)]))
        .unwrap();
    doc.set_custom_text("").unwrap();
    assert!(doc.undo().unwrap().is_none());
    drop(doc);
    // Opening (and no-op calls) write nothing.
    drop(open(dir).unwrap());
    assert_eq!(log_len(dir), 0);
    assert!(peers_on_disk(dir).is_empty());

    let mut doc = open(dir).unwrap();
    doc.insert(0, "one").unwrap();
    doc.insert(3, " two").unwrap();
    drop(doc);
    let peers = peers_on_disk(dir);
    assert_eq!(peers.len(), 1);
    assert_eq!(peers.values().next(), Some(&entry(&me())));

    // A new session is a new peer.
    let mut doc = open(dir).unwrap();
    doc.insert(7, "!").unwrap();
    drop(doc);
    let peers = peers_on_disk(dir);
    assert_eq!(peers.len(), 2);
    assert!(peers.values().all(|e| *e == entry(&me())));
}

#[test]
fn every_kind_of_first_change_registers() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut doc = create(dir).unwrap();
    doc.insert(0, "abc").unwrap();
    let cp = doc.checkpoint(Some("abc"), at(0)).unwrap();
    doc.delete(0, 3).unwrap();
    drop(doc);
    let base = peers_on_disk(dir).len();

    type FirstChange = fn(&mut Document, CheckpointId);
    let firsts: [(&str, FirstChange); 8] = [
        ("insert", |d, _| d.insert(0, "xy").unwrap()),
        ("delete", |d, _| d.delete(0, 1).unwrap()),
        ("apply_changes", |d, _| {
            d.apply_changes(&[ch(0, 0, "z")]).unwrap()
        }),
        ("map_update", |d, _| {
            d.map_update(MapName::Layout, &changes(&[("k", Some(json!(1)))]))
                .unwrap()
        }),
        ("map_replace", |d, _| {
            let entries: Entries = [("k".to_owned(), json!(2))].into();
            d.map_replace(MapName::Layout, &entries).unwrap()
        }),
        ("set_custom_text", |d, _| {
            d.set_custom_text("#let a = 1").unwrap()
        }),
        ("set_bibliography", |d, _| {
            d.set_bibliography(
                "@misc{a, title = {A}}
",
            )
            .unwrap()
        }),
        ("restore", |d, cp| {
            d.restore(cp).unwrap();
        }),
    ];
    for (i, (name, first)) in firsts.iter().enumerate() {
        let mut doc = open(dir).unwrap();
        first(&mut doc, cp);
        // Only the change itself is an undo step.
        assert!(doc.undo().unwrap().is_some(), "{name}");
        assert!(!doc.can_undo(), "{name}");
        doc.redo().unwrap();
        drop(doc);
        assert_eq!(peers_on_disk(dir).len(), base + i + 1, "{name}");
    }
    assert_eq!(open(dir).unwrap().text(), "abc");
}

#[test]
fn registration_is_not_undoable() {
    let (tmp, mut doc) = new_doc();
    doc.insert(0, "abc").unwrap();
    assert_eq!(doc.undo().unwrap().unwrap(), [ch(0, 3, "")]);
    assert_eq!(doc.text(), "");
    assert!(!doc.can_undo());
    assert!(doc.redo().unwrap().is_some());
    assert_eq!(doc.text(), "abc");
    doc.undo().unwrap();
    // The entry is still there: the next edit (not registering again) is
    // attributed.
    doc.insert(0, "x").unwrap();
    assert_eq!(author(&doc, 0), by(&me()));
    drop(doc);
    assert_eq!(peers_on_disk(tmp.path()).len(), 1);
}

#[test]
fn authors_on_two_devices() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let (a, b) = (who(1), who(2));
    let mut doc = Document::create(dir, &a).unwrap();
    doc.insert(0, "Hello").unwrap();
    drop(doc);
    let mut doc = Document::open(dir, &b).unwrap();
    doc.apply_changes(&[ch(5, 5, " 🌍 world")]).unwrap();
    drop(doc);

    let doc = Document::open(dir, &a).unwrap();
    assert_eq!(doc.text(), "Hello 🌍 world");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    for (pos, id) in [(0, &a), (4, &a), (5, &b), (6, &b), (8, &b), (13, &b)] {
        let found = doc.author_at(pos).unwrap().unwrap();
        assert_eq!(
            (&found.user, &found.device),
            (&id.user, &id.device),
            "{pos}"
        );
        let time = found.time.unwrap();
        assert!((now - 60..=now + 60).contains(&time), "{time}");
    }
    assert!(matches!(doc.author_at(7), Err(Error::InvalidChange(_))));
    assert!(matches!(doc.author_at(14), Err(Error::OutOfRange { .. })));
    assert!(matches!(doc.author_at(99), Err(Error::OutOfRange { .. })));
    drop(doc);
    let (_tmp, empty) = new_doc();
    assert!(matches!(empty.author_at(0), Err(Error::OutOfRange { .. })));
}

#[test]
fn restore_leaves_peers_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut doc = create(dir).unwrap();
    doc.insert(0, "abc").unwrap();
    let cp = doc.checkpoint(Some("abc"), at(0)).unwrap();
    drop(doc);

    // This session registers after the checkpoint.
    let mut doc = open(dir).unwrap();
    doc.insert(3, "def").unwrap();
    assert_eq!(doc.restore(cp).unwrap(), [ch(3, 6, "")]);
    assert_eq!(doc.text(), "abc");
    doc.insert(3, "x").unwrap();
    assert_eq!(author(&doc, 3), by(&me()));
    doc.undo().unwrap();
    assert_eq!(doc.undo().unwrap().unwrap(), [ch(3, 3, "def")]);
    assert_eq!(doc.text(), "abcdef");
    assert_eq!(author(&doc, 3), by(&me()));
    doc.end_undo_step();
    doc.delete(0, 6).unwrap();
    drop(doc);
    assert_eq!(peers_on_disk(dir).len(), 2);

    // A restore as the first change registers, and its insertions are the
    // restoring session's.
    let mut doc = open(dir).unwrap();
    assert_eq!(doc.restore(cp).unwrap(), [ch(0, 0, "abc")]);
    assert_eq!(author(&doc, 0), by(&me()));
    assert_eq!(doc.undo().unwrap().unwrap(), [ch(0, 3, "")]);
    assert!(!doc.can_undo());
    drop(doc);
    assert_eq!(peers_on_disk(dir).len(), 3);
}

#[test]
fn unknown_and_invalid_peers_have_no_author() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut doc = create(dir).unwrap();
    doc.insert(0, "mine").unwrap();
    drop(doc);

    // Device 3, not Lethora: peers 77-80 write a character each, with a
    // garbage entry, an entry with a bad user, no entry and a valid one.
    let entry = |user: &str, device: &str| {
        LoroValue::from(HashMap::from([
            ("user", LoroValue::from(user)),
            ("device", LoroValue::from(device)),
            ("extra", LoroValue::from(1)),
        ]))
    };
    let third = who(3);
    let (user, device) = (third.user.as_str(), third.device.as_str());
    let mut log = Vec::new();
    for (peer, c, value) in [
        (77, "Z", Some(LoroValue::from("garbage"))),
        (78, "Y", Some(entry("nope", device))),
        (79, "W", None),
        (80, "V", Some(entry(user, device))),
    ] {
        let x = LoroDoc::new();
        x.set_peer_id(peer).unwrap();
        x.get_text("content").insert(0, c).unwrap();
        if let Some(value) = value {
            x.get_map("peers").insert(&peer.to_string(), value).unwrap();
        }
        x.commit();
        log.extend(frame(&x.export(ExportMode::all_updates()).unwrap()));
    }
    fs::write(own(dir, &third, "log"), log).unwrap();

    let doc = open(dir).unwrap();
    let text = doc.text();
    for c in ["Z", "Y", "W"] {
        assert_eq!(author(&doc, utf16_of(&text, c)), None, "{c}");
    }
    assert_eq!(author(&doc, utf16_of(&text, "V")), by(&third));
    assert_eq!(author(&doc, utf16_of(&text, "m")), by(&me()));
}

/// `authors_in(from, to)` as (from, to, user) per run.
fn runs(doc: &Document, from: usize, to: usize) -> Vec<(usize, usize, Option<UserId>)> {
    doc.authors_in(from, to)
        .into_iter()
        .map(|r| (r.from, r.to, r.user))
        .collect()
}

#[test]
fn authors_in_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let (a, b) = (who(1), who(2));
    let (ua, ub) = (Some(a.user.clone()), Some(b.user.clone()));
    // Two inserts of A in one session: one run.
    let mut doc = Document::create(dir, &a).unwrap();
    doc.insert(0, "Hello").unwrap();
    doc.insert(5, " world").unwrap();
    assert_eq!(runs(&doc, 0, 11), [(0, 11, ua.clone())]);
    assert!(doc.authors_in(0, 0).is_empty());
    drop(doc);

    // B writes into A's text and deletes a character inside A's "world".
    let mut doc = Document::open(dir, &b).unwrap();
    doc.apply_changes(&[ch(5, 5, " 🌍 big")]).unwrap();
    doc.apply_changes(&[ch(14, 15, "")]).unwrap();
    assert_eq!(doc.text(), "Hello 🌍 big wrld");
    drop(doc);

    let mut doc = Document::open(dir, &a).unwrap();
    let all = [
        (0, 5, ua.clone()),
        (5, 12, ub.clone()),
        (12, 17, ua.clone()),
    ];
    assert_eq!(runs(&doc, 0, 17), all);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    for run in doc.authors_in(0, 17) {
        let time = run.time.unwrap();
        assert!((now - 60..=now + 60).contains(&time), "{time}");
    }
    // Clipped to the text, widened to whole characters (🌍 is 6..8).
    assert_eq!(
        runs(&doc, 3, 999),
        [
            (3, 5, ua.clone()),
            (5, 12, ub.clone()),
            (12, 17, ua.clone())
        ]
    );
    assert_eq!(runs(&doc, 7, 7), []);
    assert_eq!(runs(&doc, 7, 8), [(6, 8, ub.clone())]);
    assert_eq!(runs(&doc, 6, 7), [(6, 8, ub.clone())]);
    assert_eq!(runs(&doc, 13, 2), []);
    assert_eq!(runs(&doc, 40, 50), []);

    // Text that undo brings back is inserted anew: the undoing user's.
    doc.apply_changes(&[ch(8, 12, "")]).unwrap();
    assert_eq!(
        runs(&doc, 0, 13),
        [(0, 5, ua.clone()), (5, 8, ub.clone()), (8, 13, ua.clone())]
    );
    doc.undo().unwrap().unwrap();
    assert_eq!(doc.text(), "Hello 🌍 big wrld");
    let users: Vec<_> = doc
        .authors_in(0, 17)
        .into_iter()
        .map(|r| (r.from, r.to, r.user))
        .collect();
    assert_eq!(users[..2], [(0, 5, ua.clone()), (5, 8, ub.clone())]);
    assert!(users[2..].iter().all(|(_, _, u)| *u == ua), "{users:?}");
    assert_eq!(users.last().unwrap().1, 17);
    drop(doc);

    // A peer without a `peers` entry: unknown (and no time without one).
    let x = LoroDoc::new();
    x.set_peer_id(79).unwrap();
    x.get_text("content").insert(0, "W").unwrap();
    x.commit();
    fs::write(
        own(dir, &who(3), "log"),
        frame(&x.export(ExportMode::all_updates()).unwrap()),
    )
    .unwrap();
    let doc = Document::open(dir, &a).unwrap();
    let w = utf16_of(&doc.text(), "W");
    let unknown = doc.authors_in(w, w + 1);
    assert_eq!(unknown.len(), 1);
    assert_eq!(
        (
            unknown[0].from,
            unknown[0].to,
            &unknown[0].user,
            unknown[0].time
        ),
        (w, w + 1, &None, None)
    );
    // Every character is in exactly one run, runs don't touch their like.
    let all = doc.authors_in(0, doc.len_utf16());
    assert_eq!(all.first().unwrap().from, 0);
    assert_eq!(all.last().unwrap().to, doc.len_utf16());
    for pair in all.windows(2) {
        assert_eq!(pair[0].to, pair[1].from);
        assert!((&pair[0].user, pair[0].time) != (&pair[1].user, pair[1].time));
    }
}

// ---- Named checkpoints in the document ----

/// The checkpoints in `id`'s own checkpoint file.
fn own_checkpoints(dir: &Path, id: &Identity) -> Vec<Checkpoint> {
    let path = dir
        .join("checkpoints")
        .join(format!("{}.json", id.device.as_str()));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// Writes `id`'s own checkpoint file.
fn write_own_checkpoints(dir: &Path, id: &Identity, checkpoints: &[Checkpoint]) {
    let path = dir
        .join("checkpoints")
        .join(format!("{}.json", id.device.as_str()));
    fs::write(path, serde_json::to_vec(checkpoints).unwrap()).unwrap();
}

/// A `checkpoints` map entry as Lethora writes it.
fn named_value(name: &str, created: i64, version: Vec<u8>) -> LoroValue {
    LoroValue::from(HashMap::from([
        ("name", LoroValue::from(name)),
        ("created", LoroValue::from(created)),
        ("version", LoroValue::from(version)),
        ("user", LoroValue::from(me().user.as_str())),
        ("device", LoroValue::from(me().device.as_str())),
    ]))
}

#[test]
fn named_checkpoints_are_shared_automatic_ones_are_not() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let (a, b) = (who(1), who(2));
    let mut doc = Document::create(dir, &a).unwrap();
    doc.insert(0, "one").unwrap();
    let named = doc.checkpoint(Some("First"), at(0)).unwrap();
    doc.insert(3, " two").unwrap();
    let auto = doc.checkpoint(None, at(60)).unwrap();
    assert_eq!(doc.checkpoints().len(), 2);
    drop(doc);
    // A's file holds the automatic one only.
    let file = own_checkpoints(dir, &a);
    assert_eq!(file.len(), 1);
    assert_eq!((file[0].id, file[0].auto), (auto, true));

    let mut doc = Document::open(dir, &b).unwrap();
    let cps = doc.checkpoints();
    assert_eq!(cps.len(), 1);
    assert_eq!(cps[0].id, named);
    assert_eq!(
        (cps[0].name.as_deref(), cps[0].auto),
        (Some("First"), false)
    );
    assert_eq!(cps[0].created, unix(at(0)));
    assert_eq!(doc.text_at(named).unwrap(), "one");
    assert!(matches!(doc.text_at(auto), Err(Error::NotFound(_))));
    assert!(matches!(doc.restore(auto), Err(Error::NotFound(_))));
    assert_eq!(doc.restore(named).unwrap(), [ch(3, 7, "")]);
    assert_eq!(doc.text(), "one");
    // B's own automatic checkpoint is B's alone.
    doc.insert(3, "!").unwrap();
    let b_auto = doc.checkpoint(None, at(120)).unwrap();
    drop(doc);
    let doc = Document::open(dir, &a).unwrap();
    assert_eq!(doc.text(), "one!");
    let ids: Vec<CheckpointId> = doc.checkpoints().iter().map(|c| c.id).collect();
    assert_eq!(ids, [named, auto]);
    assert!(!ids.contains(&b_auto));
}

fn unix(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).unwrap().as_secs()
}

#[test]
fn restore_keeps_newer_named_checkpoints() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut doc = create(dir).unwrap();
    doc.insert(0, "v1").unwrap();
    let old = doc.checkpoint(Some("Old"), at(0)).unwrap();
    drop(doc);
    // A new session (a new peer) adds text and a newer checkpoint.
    let mut doc = open(dir).unwrap();
    doc.insert(2, " v2").unwrap();
    let new = doc.checkpoint(Some("New"), at(60)).unwrap();
    doc.end_undo_step();
    doc.insert(5, " v3").unwrap();
    let listed = doc.checkpoints();
    assert_eq!(listed.len(), 2);

    assert_eq!(doc.restore(old).unwrap(), [ch(2, 8, "")]);
    assert_eq!(doc.text(), "v1");
    assert_eq!(doc.checkpoints(), listed, "newer checkpoint kept");
    assert_eq!(doc.text_at(new).unwrap(), "v1 v2");
    doc.undo().unwrap();
    assert_eq!(doc.text(), "v1 v2 v3");
    assert_eq!(doc.checkpoints(), listed);
    doc.redo().unwrap();
    assert_eq!(doc.text(), "v1");
    doc.restore(new).unwrap();
    assert_eq!(doc.text(), "v1 v2");
    drop(doc);
    assert_eq!(peers_on_disk(dir).len(), 2, "peers kept");
    let doc = open(dir).unwrap();
    assert_eq!(doc.checkpoints(), listed);
    assert_eq!(doc.text(), "v1 v2");
}

#[test]
fn creating_a_named_checkpoint_is_not_undoable() {
    let (_tmp, mut doc) = new_doc();
    doc.insert(0, "abc").unwrap();
    let cp = doc.checkpoint(Some("abc"), at(0)).unwrap();
    // Typing right after it is a step of its own.
    doc.insert(3, "d").unwrap();
    assert_eq!(doc.undo().unwrap().unwrap(), [ch(3, 4, "")]);
    assert_eq!(doc.undo().unwrap().unwrap(), [ch(0, 3, "")]);
    assert!(!doc.can_undo());
    assert_eq!(doc.text(), "");
    assert_eq!(doc.checkpoints().len(), 1);
    assert_eq!(doc.text_at(cp).unwrap(), "abc");
    doc.redo().unwrap();
    doc.redo().unwrap();
    assert_eq!(doc.text(), "abcd");
    assert_eq!(doc.checkpoints()[0].id, cp);
}

#[test]
fn auto_checkpoints_ignore_bookkeeping_changes() {
    let (_tmp, mut doc) = new_doc();
    doc.insert(0, "a").unwrap();
    doc.checkpoint(Some("named"), at(0)).unwrap();
    // Only the checkpoint entry was written since: unchanged.
    assert_eq!(doc.maybe_auto_checkpoint(at(600)).unwrap(), None);
    doc.insert(1, "b").unwrap();
    assert!(doc.maybe_auto_checkpoint(at(600)).unwrap().is_some());
    assert_eq!(doc.maybe_auto_checkpoint(at(1200)).unwrap(), None);
}

#[test]
fn checkpoint_ids_are_js_safe_and_unique() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut ids = Vec::new();
    for (n, id) in [who(1), who(2)].iter().enumerate() {
        let mut doc = if n == 0 {
            Document::create(dir, id).unwrap()
        } else {
            Document::open(dir, id).unwrap()
        };
        for i in 0..5 {
            doc.insert(0, "x").unwrap();
            ids.push(doc.checkpoint(Some("n"), at(i)).unwrap());
            doc.insert(0, "y").unwrap();
            ids.push(doc.checkpoint(None, at(i)).unwrap());
        }
    }
    let unique: BTreeSet<u64> = ids.iter().map(|id| id.0).collect();
    assert_eq!(unique.len(), 20);
    assert!(
        unique.iter().all(|&id| id > 0 && id < 1 << 53),
        "{unique:?}"
    );
    // Every named one is listed on the second device, besides its own autos.
    let doc = Document::open(dir, &who(3)).unwrap();
    assert_eq!(doc.checkpoints().len(), 10);
}

#[test]
fn named_checkpoints_move_from_the_own_file_on_open() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut doc = create(dir).unwrap();
    doc.insert(0, "v1").unwrap();
    let v1 = doc.checkpoint(None, at(0)).unwrap();
    doc.insert(2, " v2").unwrap();
    let v2 = doc.checkpoint(None, at(60)).unwrap();
    let autos = doc.checkpoints();
    drop(doc);
    // A file as part 1a wrote it: named and automatic checkpoints, small ids.
    let version = |id| autos.iter().find(|c| c.id == id).unwrap().version.clone();
    let file = vec![
        Checkpoint {
            id: CheckpointId(1),
            name: Some("Draft".into()),
            auto: false,
            created: unix(at(0)),
            version: version(v1),
        },
        Checkpoint {
            id: CheckpointId(2),
            name: None,
            auto: true,
            created: unix(at(60)),
            version: version(v2),
        },
    ];
    write_own_checkpoints(dir, &me(), &file);

    let doc = open(dir).unwrap();
    assert_eq!(doc.checkpoints(), file);
    assert_eq!(doc.text_at(CheckpointId(1)).unwrap(), "v1");
    drop(doc);
    assert_eq!(own_checkpoints(dir, &me()), &file[1..]);
    // In the document: another device sees it.
    let other = Document::open(dir, &who(2)).unwrap();
    assert_eq!(other.checkpoints(), &file[..1]);
    drop(other);
    // Reopening doesn't duplicate it, nor does a crash before the file was
    // rewritten (the old file back).
    assert_eq!(open(dir).unwrap().checkpoints(), file);
    write_own_checkpoints(dir, &me(), &file);
    assert_eq!(open(dir).unwrap().checkpoints(), file);
    assert_eq!(own_checkpoints(dir, &me()), &file[1..]);

    // Device 2's 1a file has a different named checkpoint with the same id:
    // it gets a new id, both are kept.
    let mine = Checkpoint {
        name: Some("Mine".into()),
        created: unix(at(30)),
        ..file[0].clone()
    };
    write_own_checkpoints(dir, &who(2), std::slice::from_ref(&mine));
    let doc = Document::open(dir, &who(2)).unwrap();
    let cps = doc.checkpoints();
    assert_eq!(cps.len(), 2);
    assert_eq!(cps[0], file[0]);
    assert_eq!(cps[1].name.as_deref(), Some("Mine"));
    assert_ne!(cps[1].id, CheckpointId(1));
    assert_eq!(doc.text_at(cps[1].id).unwrap(), "v1");
    drop(doc);
    assert!(own_checkpoints(dir, &who(2)).is_empty());
    assert_eq!(Document::open(dir, &who(2)).unwrap().checkpoints(), cps);
}

#[test]
fn foreign_and_invalid_named_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut doc = create(dir).unwrap();
    doc.insert(0, "mine").unwrap();
    drop(doc);

    // Peer 12345's change, whose history arrives later (device 4).
    let late = LoroDoc::new();
    late.set_peer_id(12345).unwrap();
    late.get_text("content").insert(0, "late ").unwrap();
    late.commit();
    let late_version = late.oplog_frontiers().encode();

    // Device 3, not Lethora, writes entries.
    let x = LoroDoc::new();
    x.set_peer_id(90).unwrap();
    let map = x.get_map("checkpoints");
    let valid = |name| named_value(name, 1_700_000_000, Frontiers::default().encode());
    for (key, value) in [
        ("11", valid("valid")),
        ("5", named_value("late", 1_700_000_000, late_version)),
        ("6", LoroValue::from("garbage")),
        (
            "7",
            LoroValue::from(HashMap::from([("name", LoroValue::from("no version"))])),
        ),
        ("8", named_value("bad version", 1, vec![1, 2, 3])),
        ("9", named_value("negative time", -1, Vec::new())),
        ("0", valid("zero")),
        ("9007199254740992", valid("too big")),
        ("012", valid("not canonical")),
        ("x", valid("not a number")),
    ] {
        map.insert(key, value).unwrap();
    }
    x.commit();
    fs::write(
        own(dir, &who(3), "log"),
        frame(&x.export(ExportMode::all_updates()).unwrap()),
    )
    .unwrap();

    let mut doc = open(dir).unwrap();
    let cps = doc.checkpoints();
    assert_eq!(cps.len(), 1, "{cps:?}");
    assert_eq!(cps[0].id, CheckpointId(11));
    assert_eq!(doc.text_at(CheckpointId(11)).unwrap(), "");
    assert!(matches!(
        doc.text_at(CheckpointId(5)),
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        doc.restore(CheckpointId(6)),
        Err(Error::NotFound(_))
    ));
    // New ids avoid every key, valid or not.
    let id = doc.checkpoint(Some("new"), at(0)).unwrap();
    assert!(![0, 5, 6, 7, 8, 9, 11, 12].contains(&id.0));
    drop(doc);

    // The history arrives: the entry is listed.
    fs::write(
        own(dir, &who(4), "log"),
        frame(&late.export(ExportMode::all_updates()).unwrap()),
    )
    .unwrap();
    let doc = open(dir).unwrap();
    let ids: Vec<CheckpointId> = doc.checkpoints().iter().map(|c| c.id).collect();
    assert_eq!(ids.len(), 3);
    assert!(ids.contains(&CheckpointId(5)));
    assert_eq!(doc.text_at(CheckpointId(5)).unwrap(), "late ");
}

#[test]
fn named_checkpoints_are_durable_at_once() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("p");
    let copy = tmp.path().join("copy");
    let cp = {
        let mut doc = create(&dir).unwrap();
        doc.insert(0, "kept").unwrap();
        let cp = doc.checkpoint(Some("now"), at(0)).unwrap();
        // A crash right now: copy the files while the document is open.
        let id = me().device;
        let id = id.as_str();
        for name in [
            "project.json".to_owned(),
            format!("main/{id}.snapshot"),
            format!("main/{id}.log"),
            format!("checkpoints/{id}.json"),
        ] {
            fs::create_dir_all(copy.join(&name).parent().unwrap()).unwrap();
            fs::copy(dir.join(&name), copy.join(&name)).unwrap();
        }
        cp
    };
    for dir in [&dir, &copy] {
        let doc = open(dir).unwrap();
        assert_eq!(doc.checkpoints().len(), 1);
        assert_eq!(doc.text_at(cp).unwrap(), "kept");
    }
}

// ---- Bibliography and its references.bib mirror ----

/// A BibLaTeX entry with key `key`.
fn source(key: &str) -> String {
    format!("@misc{{{key},\n  title = {{About {key}}},\n  year = {{2020}},\n}}\n")
}

fn sources(keys: &[&str]) -> String {
    keys.iter()
        .map(|k| source(k))
        .collect::<Vec<_>>()
        .join("\n")
}

fn references(dir: &Path) -> PathBuf {
    dir.join("references.bib")
}

/// The marker line and the text of a Lethora mirror.
fn mirror_parts(dir: &Path) -> (String, String) {
    let file = fs::read_to_string(references(dir)).unwrap();
    let (marker, text) = file.split_once('\n').unwrap();
    (marker.to_owned(), text.to_owned())
}

/// A file a Lethora mirror of `text` could be: a valid marker (with `note`
/// after the crc) and the text.
fn marked(text: &str, note: &str) -> String {
    format!(
        "% Lethora mirror crc32={:08x}{note}\n{text}",
        crc32fast::hash(text.as_bytes())
    )
}

fn device_state(dir: &Path, id: &Identity) -> Option<Value> {
    let path = dir
        .join("state")
        .join(format!("{}.json", id.device.as_str()));
    fs::read(path)
        .ok()
        .map(|json| serde_json::from_slice(&json).unwrap())
}

#[test]
fn bibliography_round_trip_and_own_undo_steps() {
    let (tmp, mut doc) = new_doc();
    assert_eq!(doc.bibliography(), "");
    let (a, ab) = (sources(&["a"]), sources(&["a", "b"]));
    doc.insert(0, "See @a.").unwrap();
    // Within the merge interval, yet a step of its own.
    doc.set_bibliography(&a).unwrap();
    assert_eq!(doc.bibliography(), a);
    doc.set_bibliography(&ab).unwrap();
    doc.set_bibliography(&ab).unwrap();
    assert_eq!(
        doc.undo().unwrap(),
        Some(vec![]),
        "an import changes no text"
    );
    assert_eq!(doc.bibliography(), a);
    assert_eq!(doc.undo().unwrap(), Some(vec![]));
    assert_eq!(doc.bibliography(), "");
    assert_eq!(doc.text(), "See @a.");
    assert_eq!(doc.undo().unwrap().unwrap(), [ch(0, 7, "")]);
    assert!(!doc.can_undo());
    for _ in 0..3 {
        doc.redo().unwrap();
    }
    assert_eq!(doc.text(), "See @a.");
    assert_eq!(doc.bibliography(), ab);

    // Typing right after an import is a step of its own too.
    doc.insert(7, "!").unwrap();
    assert_eq!(doc.undo().unwrap().unwrap(), [ch(7, 8, "")]);
    assert_eq!(doc.bibliography(), ab);

    // Stored without a byte order mark and with `\n` line breaks.
    doc.set_bibliography(&format!("\u{feff}{}", ab.replace('\n', "\r\n")))
        .unwrap();
    assert_eq!(doc.bibliography(), ab);
    assert!(doc.can_redo(), "nothing changed, so no new step");

    let big = "%".repeat(MAX_BIBLIOGRAPHY_BYTES + 1);
    assert!(matches!(
        doc.set_bibliography(&big),
        Err(Error::InvalidChange(_))
    ));
    assert_eq!(doc.bibliography(), ab);

    drop(doc);
    let doc = open(tmp.path()).unwrap();
    assert_eq!(doc.bibliography(), ab);
}

#[test]
fn checkpoints_cover_the_bibliography() {
    let (_tmp, mut doc) = new_doc();
    let abc = sources(&["a", "b", "c"]);
    doc.set_bibliography(&abc).unwrap();
    let cp = doc.checkpoint(Some("abc"), at(0)).unwrap();
    let edited = abc.replace("About b", "On b");
    doc.set_bibliography(&edited).unwrap();
    assert_eq!(doc.bibliography(), edited);
    assert_eq!(doc.restore(cp).unwrap(), []);
    assert_eq!(doc.bibliography(), abc);
    assert_eq!(doc.undo().unwrap(), Some(vec![]));
    assert_eq!(doc.bibliography(), edited);
}

#[test]
fn the_mirror_is_written_on_autosave_with_a_marker() {
    let (tmp, mut doc) = new_doc();
    let dir = tmp.path();
    doc.insert(0, "x").unwrap();
    doc.autosave().unwrap();
    assert!(!references(dir).exists(), "empty bibliography, no file");
    assert!(device_state(dir, &me()).is_none());

    let a = sources(&["a"]);
    doc.set_bibliography(&a).unwrap();
    doc.autosave().unwrap();
    let (marker, text) = mirror_parts(dir);
    assert_eq!(text, a);
    assert!(
        marker.starts_with(&format!(
            "% Lethora mirror crc32={:08x}; ",
            crc32fast::hash(a.as_bytes())
        )),
        "{marker}"
    );
    let state = device_state(dir, &me()).unwrap();
    assert_eq!(
        state["mirror"]["crc32"],
        json!(crc32fast::hash(a.as_bytes()))
    );
    assert_eq!(state["mirror"]["len"], json!(a.len()));
    assert!(state["mirror"]["version"].as_str().unwrap().len() >= 2);
    // A Lethora mirror is never imported, also when it differs.
    assert!(!doc.merge_bibliography_mirror().unwrap());
    let other = marked(&sources(&["z"]), "; another device");
    fs::write(references(dir), &other).unwrap();
    assert!(!doc.merge_bibliography_mirror().unwrap());
    assert_eq!(doc.bibliography(), a);

    // Not rewritten while the bibliography is unchanged, nor recreated.
    doc.insert(1, "y").unwrap();
    doc.autosave().unwrap();
    assert_eq!(fs::read_to_string(references(dir)).unwrap(), other);
    fs::remove_file(references(dir)).unwrap();
    doc.insert(2, "z").unwrap();
    doc.autosave().unwrap();
    assert!(!references(dir).exists());

    // Undo, redo and restore are bibliography changes like any other.
    let cp = doc.checkpoint(Some("a"), at(0)).unwrap();
    let ab = sources(&["a", "b"]);
    doc.set_bibliography(&ab).unwrap();
    doc.autosave().unwrap();
    assert_eq!(mirror_parts(dir).1, ab);
    doc.undo().unwrap();
    doc.autosave().unwrap();
    assert_eq!(mirror_parts(dir).1, a);
    doc.redo().unwrap();
    doc.autosave().unwrap();
    assert_eq!(mirror_parts(dir).1, ab);
    doc.restore(cp).unwrap();
    drop(doc);
    assert_eq!(mirror_parts(dir).1, a);

    // Emptied: the mirror keeps only its marker.
    let mut doc = open(dir).unwrap();
    doc.set_bibliography("").unwrap();
    doc.autosave().unwrap();
    assert_eq!(mirror_parts(dir).1, "");
}

#[test]
fn a_second_device_leaves_a_marked_mirror_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let (a, b) = (who(1), who(2));
    let text = sources(&["a"]);
    let mut doc = Document::create(dir, &a).unwrap();
    doc.set_bibliography(&text).unwrap();
    drop(doc);
    // The same text under another marker wording (say, a newer Lethora's).
    let file = marked(&text, "; written by a newer Lethora");
    fs::write(references(dir), &file).unwrap();

    let mut doc = Document::open(dir, &b).unwrap();
    assert_eq!(doc.bibliography(), text);
    assert!(!doc.merge_bibliography_mirror().unwrap());
    doc.insert(0, "typed on b").unwrap();
    doc.autosave().unwrap();
    assert_eq!(fs::read_to_string(references(dir)).unwrap(), file);
    assert!(device_state(dir, &b).is_some(), "b counts it as mirrored");
    let ab = sources(&["a", "b"]);
    doc.set_bibliography(&ab).unwrap();
    drop(doc);
    assert_eq!(mirror_parts(dir).1, ab);
    assert_eq!(Document::open(dir, &a).unwrap().bibliography(), ab);
}

#[test]
fn outside_edits_are_merged_as_an_undo_step() {
    let (tmp, mut doc) = new_doc();
    let dir = tmp.path();
    let (a, ab) = (sources(&["a"]), sources(&["a", "b"]));
    doc.set_bibliography(&a).unwrap();
    doc.autosave().unwrap();
    // Zotero exports without the marker.
    fs::write(references(dir), &ab).unwrap();
    assert!(doc.merge_bibliography_mirror().unwrap());
    assert_eq!(doc.bibliography(), ab);
    // Written back with a marker at once.
    assert_eq!(mirror_parts(dir).1, ab);
    assert!(!doc.merge_bibliography_mirror().unwrap());
    assert_eq!(doc.undo().unwrap(), Some(vec![]));
    assert_eq!(doc.bibliography(), a);
    doc.autosave().unwrap();
    assert_eq!(mirror_parts(dir).1, a);

    // A hand edit that kept a (now stale) marker; byte order mark and CRLF.
    let abc = sources(&["a", "b", "c"]);
    let (marker, _) = mirror_parts(dir);
    fs::write(
        references(dir),
        format!("\u{feff}{marker}\r\n{}", abc.replace('\n', "\r\n")),
    )
    .unwrap();
    assert!(doc.merge_bibliography_mirror().unwrap());
    assert_eq!(doc.bibliography(), abc);
    assert_eq!(mirror_parts(dir).1, abc);

    // Unchanged text in another form: nothing to merge, the marker is back.
    fs::write(references(dir), abc.replace('\n', "\r\n")).unwrap();
    assert!(!doc.merge_bibliography_mirror().unwrap());
    assert_eq!(mirror_parts(dir).1, abc);

    // Unreadable files fail and change nothing.
    fs::write(references(dir), b"@misc{x, title = {\xff}}").unwrap();
    assert!(matches!(
        doc.merge_bibliography_mirror(),
        Err(Error::InvalidChange(_))
    ));
    let big = "%".repeat(MAX_BIBLIOGRAPHY_BYTES + 1);
    fs::write(references(dir), big).unwrap();
    assert!(matches!(
        doc.merge_bibliography_mirror(),
        Err(Error::InvalidChange(_))
    ));
    assert_eq!(doc.bibliography(), abc);
}

#[test]
fn concurrent_changes_merge_three_way() {
    let (tmp, mut doc) = new_doc();
    let dir = tmp.path();
    doc.set_bibliography(&sources(&["a", "c"])).unwrap();
    doc.autosave().unwrap();
    // Zotero adds `b` (and drops the marker) ...
    fs::write(references(dir), sources(&["a", "b", "c"])).unwrap();
    // ... while Lethora adds `d`; the autosave leaves the outside edit alone.
    doc.set_bibliography(&sources(&["a", "c", "d"])).unwrap();
    doc.autosave().unwrap();
    assert_eq!(
        fs::read_to_string(references(dir)).unwrap(),
        sources(&["a", "b", "c"])
    );
    let own_peers = peers_on_disk(dir).len();

    assert!(doc.merge_bibliography_mirror().unwrap());
    let merged = doc.bibliography();
    assert_eq!(merged, sources(&["a", "b", "c", "d"]));
    assert_eq!(mirror_parts(dir).1, merged);

    // The merge is not undoable: undo takes back Lethora's `d` only.
    assert_eq!(doc.undo().unwrap(), Some(vec![]));
    assert_eq!(doc.bibliography(), sources(&["a", "b", "c"]));
    drop(doc);

    // The fork's peer is registered to this identity; every peer is known.
    let peers = peers_on_disk(dir);
    assert_eq!(peers.len(), own_peers + 1);
    assert!(peers.values().all(|e| *e == entry(&me())));
    let loro = on_disk(dir);
    for peer in loro.oplog_vv().keys() {
        assert!(peers.contains_key(&peer.to_string()), "{peer}");
    }
    assert_eq!(open(dir).unwrap().bibliography(), sources(&["a", "b", "c"]));
}

#[test]
fn projects_with_a_plain_references_bib_migrate() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut doc = create(dir).unwrap();
    doc.insert(0, "As @a shows.").unwrap();
    drop(doc);
    let old = sources(&["a", "b"]).replace('\n', "\r\n");
    fs::write(references(dir), &old).unwrap();

    let mut doc = open(dir).unwrap();
    // Autosaving before the merge leaves the file alone.
    doc.insert(0, "x").unwrap();
    doc.autosave().unwrap();
    assert_eq!(fs::read_to_string(references(dir)).unwrap(), old);
    assert!(doc.merge_bibliography_mirror().unwrap());
    assert_eq!(doc.bibliography(), sources(&["a", "b"]));
    assert_eq!(mirror_parts(dir).1, sources(&["a", "b"]));
    assert!(device_state(dir, &me()).is_some());
    drop(doc);
    let mut doc = open(dir).unwrap();
    assert!(!doc.merge_bibliography_mirror().unwrap());
    assert_eq!(doc.bibliography(), sources(&["a", "b"]));
}

/// The mirror's cost with a ~1 MB bibliography. Run with
/// `cargo test --release -p lethora-core -- --ignored --nocapture`.
#[test]
#[ignore]
fn bibliography_mirror_cost() {
    let entry = |k: &str| {
        format!(
            "@article{{{k},\n  author = {{Lovelace, Ada and Babbage, Charles}},\n  \
             title = {{On the analytical engine, part {k}}},\n  journal = {{Notes}},\n  \
             year = {{1843}},\n  pages = {{1--20}},\n}}\n"
        )
    };
    let bib: String = (0..6000)
        .map(|i| entry(&format!("key{i:05}")))
        .collect::<Vec<_>>()
        .join("\n");
    let (tmp, mut doc) = new_doc();
    let dir = tmp.path();
    doc.insert(0, "Text.").unwrap();
    doc.autosave().unwrap();
    doc.insert(0, "x").unwrap();
    let t = Instant::now();
    doc.autosave().unwrap();
    let empty_save = t.elapsed();

    let t = Instant::now();
    doc.set_bibliography(&bib).unwrap();
    let first_set = t.elapsed();
    let t = Instant::now();
    doc.autosave().unwrap();
    let mirror_write = t.elapsed();
    doc.insert(0, "y").unwrap();
    let t = Instant::now();
    doc.autosave().unwrap();
    let typing_save = t.elapsed();
    let t = Instant::now();
    doc.autosave().unwrap();
    let noop_save = t.elapsed();

    let one_more = bib.replacen(
        "@article{key03000",
        &format!("{}\n@article{{key03000", entry("new")),
        1,
    );
    let t = Instant::now();
    doc.set_bibliography(&one_more).unwrap();
    let small_set = t.elapsed();
    doc.autosave().unwrap();

    fs::write(references(dir), format!("{one_more}\n{}", entry("zotero"))).unwrap();
    let t = Instant::now();
    assert!(doc.merge_bibliography_mirror().unwrap());
    let direct = t.elapsed();

    fs::write(
        references(dir),
        format!("{}\n{}", doc.bibliography(), entry("zotero2")),
    )
    .unwrap();
    doc.set_bibliography(&format!("{}\n{}", entry("lethora"), doc.bibliography()))
        .unwrap();
    let t = Instant::now();
    assert!(doc.merge_bibliography_mirror().unwrap());
    let three_way = t.elapsed();
    let merged = doc.bibliography();
    assert!(merged.contains("{zotero2,") && merged.contains("{lethora,"));
    println!(
        "bibliography_mirror_cost: {} KB; autosave with an empty bibliography {empty_save:?}; \
         first set {first_set:?}; autosave writing the mirror {mirror_write:?}; autosave after \
         typing {typing_save:?}; no-op autosave {noop_save:?}; set adding one entry \
         {small_set:?}; direct merge {direct:?}; three-way merge {three_way:?}",
        bib.len() / 1024
    );
}

// ---- Assets: blobs and the index ----

/// The blob files in `dir/assets/` (names of 64 lowercase hex characters).
fn blobs(dir: &Path) -> BTreeSet<String> {
    fs::read_dir(dir.join("assets"))
        .map(|entries| {
            entries
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .filter(|n| n.len() == 64 && n.bytes().all(|b| b.is_ascii_hexdigit()))
                .collect()
        })
        .unwrap_or_default()
}

fn png_only(path: &str) -> bool {
    path.ends_with(".png")
}

#[test]
fn assets_are_stored_once_under_keyed_names() {
    let (tmp, mut doc) = new_doc();
    let dir = tmp.path();
    assert_eq!(doc.asset_blob_name(b"pixels"), None, "no key yet");
    assert!(doc.assets().is_empty());
    doc.put_asset("assets/a.png", b"pixels").unwrap();
    doc.put_asset("assets/b.png", b"pixels").unwrap();
    let blob = doc.asset_blob("assets/a.png").unwrap();
    assert_eq!(doc.asset_blob("assets/b.png").as_ref(), Some(&blob));
    assert_eq!(doc.asset_blob_name(b"pixels").as_ref(), Some(&blob));
    assert_eq!(blobs(dir), BTreeSet::from([blob.clone()]));
    assert_eq!(fs::read(dir.join("assets").join(&blob)).unwrap(), b"pixels");
    assert_eq!(
        doc.assets(),
        BTreeMap::from([
            ("assets/a.png".to_owned(), blob.clone()),
            ("assets/b.png".to_owned(), blob.clone()),
        ])
    );
    assert_eq!(doc.asset_bytes("assets/a.png").unwrap().unwrap(), b"pixels");
    assert_eq!(doc.asset_bytes("assets/none.png").unwrap(), None);
    assert!(matches!(
        doc.put_asset("", b"x"),
        Err(Error::InvalidChange(_))
    ));
    assert!(matches!(
        doc.put_asset(&"a".repeat(300), b"x"),
        Err(Error::InvalidChange(_))
    ));

    // Keyed: the same bytes get another name in another project.
    let (_other_tmp, mut other) = new_doc();
    other.put_asset("assets/a.png", b"pixels").unwrap();
    assert_ne!(other.asset_blob("assets/a.png").unwrap(), blob);

    // Kept across reopening, key included.
    drop(doc);
    let doc = open(dir).unwrap();
    assert_eq!(doc.asset_blob("assets/a.png").as_ref(), Some(&blob));
    assert_eq!(doc.asset_blob_name(b"pixels").as_ref(), Some(&blob));
}

#[test]
fn putting_an_asset_is_an_undo_step_of_its_own() {
    let (_tmp, mut doc) = new_doc();
    doc.insert(0, "Figure:").unwrap();
    // Within the merge interval, yet a step of its own.
    doc.put_asset("assets/a.png", b"one").unwrap();
    doc.put_asset("assets/a.png", b"two").unwrap();
    let (one, two) = (
        doc.asset_blob_name(b"one").unwrap(),
        doc.asset_blob_name(b"two").unwrap(),
    );
    // The same content again changes nothing.
    doc.put_asset("assets/a.png", b"two").unwrap();
    assert_eq!(doc.undo().unwrap(), Some(vec![]));
    assert_eq!(doc.asset_blob("assets/a.png"), Some(one.clone()));
    assert_eq!(doc.undo().unwrap(), Some(vec![]));
    assert_eq!(doc.asset_blob("assets/a.png"), None);
    assert_eq!(doc.text(), "Figure:");
    assert_eq!(doc.undo().unwrap().unwrap(), [ch(0, 7, "")]);
    assert!(!doc.can_undo());
    for _ in 0..3 {
        doc.redo().unwrap();
    }
    assert_eq!(doc.asset_blob("assets/a.png"), Some(two.clone()));
    // Typing right after is a step of its own too; blobs stay on disk.
    doc.insert(7, "!").unwrap();
    assert_eq!(doc.undo().unwrap().unwrap(), [ch(7, 8, "")]);
    assert_eq!(doc.asset_blob("assets/a.png"), Some(two));
    doc.undo().unwrap();
    assert_eq!(doc.asset_bytes("assets/a.png").unwrap().unwrap(), b"one");
}

#[test]
fn repointing_an_asset_changes_text_and_index_in_one_step() {
    let (_tmp, mut doc) = new_doc();
    let text = "A #image(\"assets/a.png\") B #image(\"assets/a.png\")";
    doc.insert(0, text).unwrap();
    doc.put_asset("assets/a.png", b"one").unwrap();
    let one = doc.asset_blob("assets/a.png").unwrap();
    doc.insert(doc.len(), "!").unwrap();

    // A range that doesn't read the old path changes nothing.
    for (from, to, old) in [
        (10, 22, "assets/b.png"),
        (9, 21, "assets/a.png"),
        (40, 99, ""),
    ] {
        assert!(matches!(
            doc.repoint_asset(from, to, old, "assets/c.jpg", b"two"),
            Err(Error::InvalidChange(_))
        ));
    }
    assert!(doc.asset_blob("assets/c.jpg").is_none());
    assert_eq!(doc.text(), format!("{text}!"));

    // Within the merge interval of the typing, yet a step of its own; only
    // the second reference moves, the old entry stays.
    let before = doc.text();
    let changes = doc
        .repoint_asset(35, 47, "assets/a.png", "assets/c.jpg", b"two")
        .unwrap();
    let after = "A #image(\"assets/a.png\") B #image(\"assets/c.jpg\")!";
    assert_eq!(doc.text(), after);
    assert_eq!(apply_cm(&before, &changes), after);
    assert_eq!(doc.asset_blob("assets/a.png"), Some(one.clone()));
    assert_eq!(doc.asset_bytes("assets/c.jpg").unwrap().unwrap(), b"two");

    // Typing right after is a step of its own too.
    doc.insert(0, "x").unwrap();
    doc.undo().unwrap();
    let undone = doc.undo().unwrap().unwrap();
    assert_eq!(apply_cm(after, &undone), before);
    assert_eq!(doc.text(), before);
    assert!(doc.asset_blob("assets/c.jpg").is_none());
    assert_eq!(doc.asset_blob("assets/a.png"), Some(one));
    let redone = doc.redo().unwrap().unwrap();
    assert_eq!(apply_cm(&before, &redone), after);
    assert_eq!(doc.asset_bytes("assets/c.jpg").unwrap().unwrap(), b"two");

    // The same path and content again changes nothing.
    assert_eq!(
        doc.repoint_asset(35, 47, "assets/c.jpg", "assets/c.jpg", b"two")
            .unwrap(),
        []
    );
    assert_eq!(doc.text(), after);
}

#[test]
fn staged_blobs_are_indexed_with_their_text_in_one_step() {
    let (tmp, mut doc) = new_doc();
    let dir = tmp.path();
    doc.insert(0, "A\n\nB").unwrap();
    doc.put_asset("assets/old.png", b"old").unwrap();
    let old = doc.asset_blob("assets/old.png").unwrap();
    // Staging writes the blob only.
    let one = doc.stage_blob(b"one").unwrap();
    let two = doc.stage_blob(b"two").unwrap();
    assert!(blobs(dir).contains(&one) && blobs(dir).contains(&two));
    assert_eq!(doc.assets().len(), 1);
    let entries = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(p, b)| ((*p).to_owned(), (*b).to_owned()))
            .collect()
    };

    // Refusals change nothing.
    let missing = "f".repeat(64);
    for (pos, list) in [
        (99, entries(&[("assets/a.png", &one)])),
        (0, entries(&[("assets/f/", &one)])),
        (0, entries(&[("", &one)])),
        (0, entries(&[("assets/a.png", &missing)])),
        (0, entries(&[("assets/a.png", "nope")])),
        (0, entries(&[("assets/old.png", &one)])),
        (
            0,
            entries(&[("assets/a.png", &one), ("assets/a.png", &two)]),
        ),
    ] {
        assert!(
            matches!(
                doc.insert_with_assets(pos, "x", &list),
                Err(Error::InvalidChange(_))
            ),
            "{pos} {list:?}"
        );
    }
    assert_eq!(doc.text(), "A\n\nB");
    assert_eq!(doc.assets().len(), 1);
    assert!(doc.asset_folders().is_empty());

    // Within the merge interval of the typing, yet a step of its own: new
    // entries, a folder marker, the reused entry and the text together.
    doc.insert(4, "!").unwrap();
    let before = doc.text();
    let text = "#image(\"assets/f/a.png\") #image(\"assets/old.png\")\n\n";
    let list = entries(&[
        ("assets/f/a.png", &one),
        ("assets/b.png", &two),
        ("assets/old.png", &old),
        ("assets/f/a.png", &one),
    ]);
    let changes = doc.insert_with_assets(3, text, &list).unwrap();
    let after = format!("A\n\n{text}B!");
    assert_eq!(doc.text(), after);
    assert_eq!(apply_cm(&before, &changes), after);
    assert_eq!(doc.asset_bytes("assets/f/a.png").unwrap().unwrap(), b"one");
    assert_eq!(doc.asset_bytes("assets/b.png").unwrap().unwrap(), b"two");
    assert_eq!(
        doc.asset_folders(),
        BTreeSet::from(["assets/f/".to_owned()])
    );
    let undone = doc.undo().unwrap().unwrap();
    assert_eq!(apply_cm(&after, &undone), before);
    assert_eq!(doc.assets().len(), 1);
    assert!(doc.asset_folders().is_empty());
    assert_eq!(doc.undo().unwrap().unwrap(), [ch(4, 5, "")]);
    doc.redo().unwrap();
    doc.redo().unwrap();
    assert_eq!(doc.text(), after);
    assert_eq!(doc.assets().len(), 3);

    // Only text, or entries the index holds already: still one step each.
    assert_eq!(
        doc.insert_with_assets(0, "", &entries(&[("assets/b.png", &two)]))
            .unwrap(),
        []
    );
    assert_eq!(
        doc.insert_with_assets(0, "Z", &[]).unwrap(),
        [ch(0, 0, "Z")]
    );
    doc.undo().unwrap();
    assert_eq!(doc.text(), after);
}

#[test]
fn restore_reverts_replaced_assets_but_never_removes_one() {
    let (_tmp, mut doc) = new_doc();
    let empty = doc.checkpoint(Some("empty"), at(0)).unwrap();
    doc.insert(0, "#image(\"assets/a.png\")").unwrap();
    doc.put_asset("assets/a.png", b"one").unwrap();
    let key = doc.asset_blob_name(b"x").unwrap();
    let one = doc.checkpoint(Some("one"), at(1)).unwrap();
    doc.put_asset("assets/a.png", b"two").unwrap();
    doc.put_asset("assets/b.png", b"three").unwrap();

    // The replaced image goes back; the newer entry stays.
    assert_eq!(doc.restore(one).unwrap(), []);
    assert_eq!(doc.asset_bytes("assets/a.png").unwrap().unwrap(), b"one");
    assert!(doc.asset_blob("assets/b.png").is_some());
    assert_eq!(doc.undo().unwrap(), Some(vec![]));
    assert_eq!(doc.asset_bytes("assets/a.png").unwrap().unwrap(), b"two");
    doc.redo().unwrap();
    assert_eq!(doc.asset_bytes("assets/a.png").unwrap().unwrap(), b"one");

    // A checkpoint from before the first image: the text goes back, the
    // index keeps every entry and the project key stays.
    assert_eq!(doc.restore(empty).unwrap(), [ch(0, 22, "")]);
    assert_eq!(doc.assets().len(), 2);
    assert_eq!(doc.asset_bytes("assets/a.png").unwrap().unwrap(), b"one");
    assert_eq!(doc.asset_blob_name(b"x").unwrap(), key);
    doc.undo().unwrap();
    assert_eq!(doc.text(), "#image(\"assets/a.png\")");
    assert_eq!(doc.assets().len(), 2);
}

#[test]
fn loose_assets_are_adopted_and_moved_to_the_backup() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut doc = create(dir).unwrap();
    doc.put_asset("assets/stale.png", b"indexed").unwrap();
    let indexed = doc.asset_blob("assets/stale.png").unwrap();
    drop(doc);
    let assets = dir.join("assets");
    let blob_like = "ab".repeat(32);
    for (name, bytes) in [
        ("old.png", &b"old"[..]),
        ("stale.png", b"a stale copy"),
        ("notes.txt", b"not an image"),
        (blob_like.as_str(), b"a blob"),
    ] {
        fs::write(assets.join(name), bytes).unwrap();
    }

    let mut doc = open(dir).unwrap();
    assert_eq!(
        doc.adopt_loose_assets(png_only).unwrap(),
        ["assets/old.png"]
    );
    assert!(!doc.can_undo(), "adopting is not undoable");
    assert_eq!(doc.asset_bytes("assets/old.png").unwrap().unwrap(), b"old");
    assert_eq!(doc.asset_blob("assets/stale.png"), Some(indexed));
    let backup = dir.join("backup/assets");
    assert_eq!(fs::read(backup.join("old.png")).unwrap(), b"old");
    assert_eq!(fs::read(backup.join("stale.png")).unwrap(), b"a stale copy");
    assert!(!assets.join("old.png").exists() && !assets.join("stale.png").exists());
    assert!(assets.join("notes.txt").exists() && assets.join(&blob_like).exists());

    // Again: nothing to do. A copy already adopted (as after a crash before
    // the move) only moves, under a free name.
    assert!(doc.adopt_loose_assets(png_only).unwrap().is_empty());
    fs::copy(backup.join("old.png"), assets.join("old.png")).unwrap();
    assert!(doc.adopt_loose_assets(png_only).unwrap().is_empty());
    assert_eq!(fs::read(backup.join("old-2.png")).unwrap(), b"old");
    assert!(!assets.join("old.png").exists());

    // Persisted at once.
    let saved = log_len(dir);
    drop(doc);
    assert_eq!(log_len(dir), saved);
    let doc = open(dir).unwrap();
    assert_eq!(doc.asset_bytes("assets/old.png").unwrap().unwrap(), b"old");
    assert_eq!(doc.assets().len(), 2);
}

fn set(paths: &[&str]) -> BTreeSet<String> {
    paths.iter().map(|p| p.to_string()).collect()
}

fn moves(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

#[test]
fn folder_markers_come_with_items_and_outlive_them() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut doc = create(dir).unwrap();
    doc.put_asset("assets/b.png", b"root").unwrap();
    assert!(doc.asset_folders().is_empty(), "the root needs no marker");
    // Into a folder: its marker comes in the same step; markers are not
    // assets.
    doc.put_asset("assets/figs/ch1/a.png", b"one").unwrap();
    assert_eq!(doc.asset_folders(), set(&["assets/figs/ch1/"]));
    assert_eq!(doc.assets().len(), 2);
    assert!(!doc.assets().contains_key("assets/figs/ch1/"));
    doc.undo().unwrap();
    assert!(doc.asset_folders().is_empty() && doc.assets().len() == 1);
    doc.redo().unwrap();
    assert_eq!(doc.asset_folders(), set(&["assets/figs/ch1/"]));

    // An empty folder; again changes nothing, an item path is refused.
    doc.add_asset_folder("assets/empty/").unwrap();
    doc.add_asset_folder("assets/empty/").unwrap();
    for bad in ["assets/x.png", ""] {
        assert!(matches!(
            doc.add_asset_folder(bad),
            Err(Error::InvalidChange(_))
        ));
    }
    assert!(matches!(
        doc.put_asset("assets/empty/", b"x"),
        Err(Error::InvalidChange(_))
    ));
    doc.undo().unwrap();
    assert_eq!(doc.asset_folders(), set(&["assets/figs/ch1/"]));
    doc.redo().unwrap();

    // Emptied by a move and a removal, the folder stays.
    doc.move_assets(&moves(&[("assets/figs/ch1/a.png", "assets/a.png")]))
        .unwrap();
    doc.remove_assets(&["assets/a.png".to_owned()]).unwrap();
    assert_eq!(
        doc.asset_folders(),
        set(&["assets/empty/", "assets/figs/ch1/"])
    );
    assert_eq!(doc.assets().keys().collect::<Vec<_>>(), ["assets/b.png"]);

    // Kept across reopening, and by restore.
    let cp = doc.checkpoint(Some("folders"), at(0)).unwrap();
    doc.remove_assets(&["assets/empty/".to_owned()]).unwrap();
    doc.restore(cp).unwrap();
    drop(doc);
    let doc = open(dir).unwrap();
    assert_eq!(
        doc.asset_folders(),
        set(&["assets/empty/", "assets/figs/ch1/"])
    );
}

#[test]
fn moving_assets_rewrites_every_literal_in_one_step() {
    let (_tmp, mut doc) = new_doc();
    let text = "A #image(\"assets/a.png\") 🌍 #image(\"assets/a.png\", width: 50%)\n\
                \"assets/a.png.bak\" #image(\"assets/f/b.png\") #image(\"assets/f/sub/c.png\")";
    doc.insert(0, text).unwrap();
    doc.put_asset("assets/a.png", b"a").unwrap();
    doc.put_asset("assets/f/b.png", b"b").unwrap();
    doc.put_asset("assets/f/sub/c.png", b"c").unwrap();
    let (a, b) = (
        doc.asset_blob("assets/a.png").unwrap(),
        doc.asset_blob("assets/f/b.png").unwrap(),
    );
    doc.insert(doc.len(), "!").unwrap();
    let before = doc.text();
    let index = doc.assets();
    let folders = doc.asset_folders();

    // Refused, changing nothing.
    for bad in [
        &[("assets/none.png", "assets/x.png")][..],
        &[("assets/a.png", "assets/f/b.png")],
        &[
            ("assets/a.png", "assets/x.png"),
            ("assets/a.png", "assets/y.png"),
        ],
        &[
            ("assets/a.png", "assets/x.png"),
            ("assets/f/b.png", "assets/x.png"),
        ],
        &[("assets/f/", "assets/f/g/")],
        &[("assets/f/", "assets/g.png")],
        &[("assets/a.png", "assets/a/")],
        &[("assets/a.png", "")],
    ] {
        assert!(
            matches!(doc.move_assets(&moves(bad)), Err(Error::InvalidChange(_))),
            "{bad:?}"
        );
    }
    assert_eq!(doc.text(), before);
    assert_eq!(
        (doc.assets(), doc.asset_folders()),
        (index.clone(), folders.clone())
    );

    // An item: within the merge interval of the typing, yet a step of its
    // own; only exact literals change.
    let changes = doc
        .move_assets(&moves(&[("assets/a.png", "assets/Mein Bild.png")]))
        .unwrap();
    let after = before.replace("\"assets/a.png\"", "\"assets/Mein Bild.png\"");
    assert_eq!(doc.text(), after);
    assert!(after.contains("\"assets/a.png.bak\""));
    assert_eq!(apply_cm(&before, &changes), after);
    assert_eq!(doc.asset_blob("assets/Mein Bild.png"), Some(a.clone()));
    assert_eq!(doc.asset_blob("assets/a.png"), None);
    let undone = doc.undo().unwrap().unwrap();
    assert_eq!(apply_cm(&after, &undone), before);
    assert_eq!(
        (doc.assets(), doc.asset_folders()),
        (index.clone(), folders.clone())
    );
    doc.redo().unwrap();
    assert_eq!(doc.text(), after);

    // A folder, as the app expands it: markers and items under it.
    let renamed = [
        ("assets/f/", "assets/g/"),
        ("assets/f/b.png", "assets/g/b.png"),
        ("assets/f/sub/", "assets/g/sub/"),
        ("assets/f/sub/c.png", "assets/g/sub/c.png"),
    ];
    doc.move_assets(&moves(&renamed)).unwrap();
    let moved = after.replace("assets/f/", "assets/g/");
    assert_eq!(doc.text(), moved);
    assert_eq!(doc.asset_folders(), set(&["assets/g/", "assets/g/sub/"]));
    assert_eq!(doc.asset_blob("assets/g/b.png"), Some(b.clone()));
    doc.undo().unwrap();
    assert_eq!(doc.text(), after);
    assert_eq!(doc.asset_folders(), folders);
    doc.redo().unwrap();

    // A swap and a move into a new folder (which gets its marker).
    doc.move_assets(&moves(&[
        ("assets/Mein Bild.png", "assets/g/b.png"),
        ("assets/g/b.png", "assets/h/a.png"),
    ]))
    .unwrap();
    assert_eq!(doc.asset_blob("assets/g/b.png"), Some(a));
    assert_eq!(doc.asset_blob("assets/h/a.png"), Some(b));
    assert!(doc.asset_folders().contains("assets/h/"));
    let swapped = moved
        .replace("\"assets/g/b.png\"", "\"assets/h/a.png\"")
        .replace("\"assets/Mein Bild.png\"", "\"assets/g/b.png\"");
    assert_eq!(doc.text(), swapped);
    // Moving to itself changes nothing.
    assert_eq!(
        doc.move_assets(&moves(&[("assets/h/a.png", "assets/h/a.png")]))
            .unwrap(),
        []
    );
}

#[test]
fn only_unused_assets_are_removed() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut doc = create(dir).unwrap();
    doc.insert(0, "#image(\"assets/a.png\")").unwrap();
    doc.put_asset("assets/a.png", b"a").unwrap();
    doc.put_asset("assets/f/b.png", b"b").unwrap();
    let index = doc.assets();

    for bad in [
        vec!["assets/a.png".to_owned()],
        vec!["assets/f/b.png".to_owned(), "assets/a.png".to_owned()],
        vec!["assets/none.png".to_owned()],
        vec!["assets/none/".to_owned()],
    ] {
        assert!(
            matches!(doc.remove_assets(&bad), Err(Error::InvalidChange(_))),
            "{bad:?}"
        );
    }
    assert_eq!(doc.assets(), index);

    // A folder with its items: one undo step; the blobs stay.
    doc.remove_assets(&["assets/f/".to_owned(), "assets/f/b.png".to_owned()])
        .unwrap();
    assert!(doc.asset_folders().is_empty());
    assert_eq!(doc.assets().len(), 1);
    assert_eq!(blobs(dir).len(), 2);
    doc.undo().unwrap();
    assert_eq!(doc.assets(), index);
    assert_eq!(doc.asset_folders(), set(&["assets/f/"]));
    assert_eq!(doc.text(), "#image(\"assets/a.png\")");
}

#[test]
fn asset_usage_counts_literals_and_sizes_come_from_the_storage() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut doc = create(dir).unwrap();
    doc.insert(
        0,
        "#image(\"assets/a.png\") #image(\"assets/a.png\") assets/b.png \"assets/b.png \"",
    )
    .unwrap();
    doc.set_custom_text("#image(\"assets/b.png\")").unwrap();
    doc.put_asset("assets/a.png", b"four").unwrap();
    doc.put_asset("assets/f/b.png", b"x").unwrap();
    doc.put_asset("assets/b.png", b"x").unwrap();
    doc.add_asset_folder("assets/empty/").unwrap();
    assert_eq!(
        doc.asset_usage(),
        BTreeMap::from([
            ("assets/a.png".to_owned(), 2),
            ("assets/b.png".to_owned(), 0),
            ("assets/f/b.png".to_owned(), 0),
        ])
    );
    let sizes = doc.asset_sizes().unwrap();
    assert_eq!(sizes.len(), 3);
    assert_eq!(sizes["assets/a.png"], Some(4));
    assert_eq!(sizes["assets/b.png"], Some(1));

    // A missing blob (not arrived yet) has no size.
    let blob = doc.asset_blob("assets/a.png").unwrap();
    fs::remove_file(dir.join("assets").join(&blob)).unwrap();
    assert_eq!(doc.asset_sizes().unwrap()["assets/a.png"], None);

    let storage = lethora_core::FsStorage::new(dir);
    let other = doc.asset_blob("assets/b.png").unwrap();
    use lethora_core::Storage;
    assert_eq!(storage.size(&format!("assets/{other}")).unwrap(), Some(1));
    assert_eq!(storage.size(&format!("assets/{blob}")).unwrap(), None);
    assert_eq!(storage.size("assets").unwrap(), None, "a folder");
    assert_eq!(storage.size("nowhere/x").unwrap(), None);
}

#[test]
fn invalid_index_entries_are_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    drop(create(dir).unwrap());
    // Device 3, not Lethora, writes entries.
    let x = LoroDoc::new();
    x.set_peer_id(90).unwrap();
    let map = x.get_map("assets");
    let good = "0".repeat(64);
    let upper = "AB".repeat(32);
    for (key, value) in [
        ("assets/good.png", LoroValue::from(good.as_str())),
        ("assets/upper.png", LoroValue::from(upper.as_str())),
        ("assets/short.png", LoroValue::from("abc")),
        ("assets/path.png", LoroValue::from("../../secret.png")),
        ("assets/number.png", LoroValue::from(5)),
    ] {
        map.insert(key, value).unwrap();
    }
    x.commit();
    fs::write(
        own(dir, &who(3), "log"),
        frame(&x.export(ExportMode::all_updates()).unwrap()),
    )
    .unwrap();
    let doc = open(dir).unwrap();
    assert_eq!(
        doc.assets(),
        BTreeMap::from([("assets/good.png".to_owned(), good)])
    );
    assert_eq!(doc.asset_blob("assets/path.png"), None);
    // Its blob hasn't arrived.
    assert_eq!(doc.asset_bytes("assets/good.png").unwrap(), None);
}

#[test]
fn a_second_device_sees_the_images() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut a = Document::create(dir, &who(1)).unwrap();
    a.put_asset("assets/a.png", b"from a").unwrap();
    drop(a);
    let mut b = Document::open(dir, &who(2)).unwrap();
    assert_eq!(b.asset_bytes("assets/a.png").unwrap().unwrap(), b"from a");
    // The same key: the same content is stored once.
    b.put_asset("assets/b.png", b"from a").unwrap();
    assert_eq!(blobs(dir).len(), 1);
    assert_eq!(b.asset_blob("assets/a.png"), b.asset_blob("assets/b.png"));
}
