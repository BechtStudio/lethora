use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lethora_core::{
    DeviceId, Document, Error, FsStorage, Identity, LockGuard, LogFile, MapName, ProjectCode,
    Pulled, Replica, Storage, SyncSettings, TextChange, UserId, replica_project,
    stored_sync_settings,
};
use loro::{LoroDoc, VersionVector};
use serde_json::{Value, json};

/// Device `n`, used by user `n`.
fn who(n: u8) -> Identity {
    Identity {
        device: DeviceId::parse(&format!("{n:032x}")).unwrap(),
        user: UserId::parse(&format!("{:032x}", 0x100 + n as u32)).unwrap(),
    }
}

fn fs_storage(dir: &Path) -> Box<dyn Storage> {
    Box::new(FsStorage::new(dir))
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

/// Applies `changes` to `text` with CodeMirror semantics.
fn apply_cm(text: &str, changes: &[TextChange]) -> String {
    let old: Vec<u16> = text.encode_utf16().collect();
    let mut out = Vec::with_capacity(old.len());
    let mut pos = 0;
    for c in changes {
        out.extend_from_slice(&old[pos..c.from]);
        out.extend(c.insert.encode_utf16());
        pos = c.to;
    }
    out.extend_from_slice(&old[pos..]);
    String::from_utf16(&out).unwrap()
}

fn set(map: &[(&str, Value)]) -> BTreeMap<String, Option<Value>> {
    map.iter()
        .map(|(k, v)| (k.to_string(), Some(v.clone())))
        .collect()
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot really an image";
const SECRET: &str = "Hello \u{1F600} world";

/// The sync folder of a test.
fn sync_dir(tmp: &Path) -> PathBuf {
    tmp.join("sync")
}

/// `main/<device>.<ext>` in `dir`.
fn file(dir: &Path, n: u8, ext: &str) -> PathBuf {
    dir.join("main")
        .join(format!("{}.{ext}", who(n).device.as_str()))
}

/// Device 1's project with a bit of everything, set up for sync.
fn setup(tmp: &Path) -> (Document, Replica, ProjectCode) {
    let mut a = Document::create(&tmp.join("a"), &who(1)).unwrap();
    a.insert(0, &format!("= Title\n{SECRET}\n")).unwrap();
    a.map_update(MapName::Layout, &set(&[("page", json!("a4"))]))
        .unwrap();
    a.map_update(MapName::Meta, &set(&[("title", json!("Paper"))]))
        .unwrap();
    a.map_update(MapName::Overrides, &set(&[("x", json!(1))]))
        .unwrap();
    a.set_custom_text("#set text(size: 11pt)").unwrap();
    a.set_bibliography("@book{a, title={A}}\n").unwrap();
    a.put_asset("assets/a.png", PNG).unwrap();
    a.checkpoint(Some("first"), at(0)).unwrap();
    let (replica, code) = a.sync_setup(fs_storage(&sync_dir(tmp))).unwrap();
    (a, replica, code)
}

/// Device `n` joins the synced project in a folder of its own.
fn join(tmp: &Path, n: u8, code: &ProjectCode) -> (Document, Replica) {
    Document::sync_join(
        fs_storage(&tmp.join(format!("dev{n}"))),
        fs_storage(&sync_dir(tmp)),
        code,
        &who(n),
    )
    .unwrap()
}

/// The records of a log file.
fn records(bytes: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 8 <= bytes.len() {
        let len = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        out.push(&bytes[at + 8..at + 8 + len]);
        at += 8 + len;
    }
    out
}

/// A log record holding `payload`.
fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = (payload.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(&crc32fast::hash(payload).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// The version of device `n`'s own (plain) files in project folder `dir`.
fn own_version(dir: &Path, n: u8) -> VersionVector {
    let doc = LoroDoc::new();
    doc.import(&fs::read(file(dir, n, "snapshot")).unwrap())
        .unwrap();
    let log = fs::read(file(dir, n, "log")).unwrap();
    for record in records(&log) {
        doc.import(record).unwrap();
    }
    doc.oplog_vv()
}

/// Every file below `dir`.
fn all_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(all_files(&path));
        } else {
            out.push(path);
        }
    }
    out
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn project_code_round_trip_and_typos() {
    let key: [u8; 32] = std::array::from_fn(|i| (i * 37 + 11) as u8);
    let code = ProjectCode::from_key(key);
    let shown = code.to_string();
    let groups: Vec<&str> = shown.split('-').collect();
    assert_eq!(groups.len(), 14);
    assert!(groups.iter().all(|g| g.len() == 4));
    assert_eq!(ProjectCode::parse(&shown).unwrap(), code);
    // Case, spaces and dashes don't matter; O, I and L read as digits.
    let typed = format!("  {} ", shown.to_lowercase().replace('-', " "));
    assert_eq!(ProjectCode::parse(&typed).unwrap().key(), &key);
    let aliased = shown.replace('0', "O").replace('1', "l");
    assert_eq!(ProjectCode::parse(&aliased).unwrap(), code);
    // Every single-character typo is caught.
    let compact: Vec<char> = shown.chars().filter(|&c| c != '-').collect();
    let alphabet: Vec<char> = "0123456789ABCDEFGHJKMNPQRSTVWXYZ".chars().collect();
    for i in 0..compact.len() {
        let mut typo = compact.clone();
        let at = alphabet.iter().position(|&c| c == typo[i]).unwrap();
        typo[i] = alphabet[(at + 1) % 32];
        let typo: String = typo.into_iter().collect();
        assert!(matches!(
            ProjectCode::parse(&typo),
            Err(Error::InvalidCode(_))
        ));
    }
    assert!(matches!(
        ProjectCode::parse(&shown[..shown.len() - 1]),
        Err(Error::InvalidCode(_))
    ));
    assert!(matches!(
        ProjectCode::parse(&format!("U{}", &shown[1..])),
        Err(Error::InvalidCode(_))
    ));
    assert_eq!(format!("{code:?}"), "ProjectCode(..)");
}

#[test]
fn setup_and_join_copy_everything() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, ra, code) = setup(tmp.path());
    assert_eq!(ra.code(), &code);
    assert_eq!(ra.project_id().len(), 32);
    // Nothing readable in the sync folder.
    let sync = sync_dir(tmp.path());
    assert!(sync.join("replica.json").exists());
    let blob = a.asset_blob("assets/a.png").unwrap();
    assert!(sync.join("assets").join(&blob).exists());
    for path in all_files(&sync) {
        let bytes = fs::read(&path).unwrap();
        assert!(!contains(&bytes, SECRET.as_bytes()), "{path:?}");
        assert!(!contains(&bytes, PNG), "{path:?}");
    }

    let typed = ProjectCode::parse(&code.to_string().to_lowercase()).unwrap();
    let (b, rb) = join(tmp.path(), 2, &typed);
    assert_eq!(rb.project_id(), ra.project_id());
    assert_eq!(b.text(), a.text());
    for map in [MapName::Layout, MapName::Meta, MapName::Overrides] {
        assert_eq!(b.map_entries(map), a.map_entries(map));
    }
    assert_eq!(b.custom_text(), a.custom_text());
    assert_eq!(b.bibliography(), a.bibliography());
    assert_eq!(b.checkpoints(), a.checkpoints());
    assert_eq!(b.checkpoints()[0].name.as_deref(), Some("first"));
    assert_eq!(b.assets(), a.assets());
    assert_eq!(b.asset_bytes("assets/a.png").unwrap().as_deref(), Some(PNG));
    assert!(tmp.path().join("dev2").join("project.json").exists());

    // Reopening the joined project works like any other.
    drop(b);
    drop(rb);
    let b = Document::open(&tmp.path().join("dev2"), &who(2)).unwrap();
    assert_eq!(b.text(), a.text());
}

#[test]
fn joining_adds_no_change() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, _ra, code) = setup(tmp.path());
    let (b, rb) = join(tmp.path(), 2, &code);
    drop(b);
    drop(rb);
    drop(a);
    let dev2 = tmp.path().join("dev2");
    assert_eq!(fs::metadata(file(&dev2, 2, "log")).unwrap().len(), 0);
    assert_eq!(own_version(&dev2, 2), own_version(&tmp.path().join("a"), 1));
}

#[test]
fn pull_reports_text_changes_and_parts() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut a, mut ra, code) = setup(tmp.path());
    let (mut b, mut rb) = join(tmp.path(), 2, &code);

    // "= Title\n" is 8 UTF-16 units, "Hello " 6, the emoji 2, " world" 6.
    let before = b.text();
    a.apply_changes(&[
        ch(2, 7, "Titel \u{1F389}"),
        ch(14, 16, "\u{1F642}\u{1F642}"),
        ch(22, 22, "!"),
    ])
    .unwrap();
    a.map_update(MapName::Layout, &set(&[("page", json!("letter"))]))
        .unwrap();
    a.sync_push(&mut ra).unwrap();
    let p = b.sync_pull(&mut rb).unwrap();
    assert!(p.changed && p.settings);
    assert!(!p.bibliography && !p.assets && !p.checkpoints);
    assert_eq!(p.blobs, 0);
    assert!(p.skipped.is_empty());
    assert_eq!(apply_cm(&before, &p.changes), b.text());
    assert_eq!(b.text(), a.text());
    assert_eq!(
        b.map_entries(MapName::Layout),
        a.map_entries(MapName::Layout)
    );

    // The other parts.
    a.set_bibliography("@book{b, title={B}}\n").unwrap();
    a.put_asset("assets/b.png", b"second image").unwrap();
    a.checkpoint(Some("second"), at(10)).unwrap();
    a.sync_push(&mut ra).unwrap();
    let p = b.sync_pull(&mut rb).unwrap();
    assert!(p.changed && p.bibliography && p.assets && p.checkpoints);
    assert!(!p.settings && p.changes.is_empty());
    assert_eq!(p.blobs, 1);
    assert_eq!(b.bibliography(), a.bibliography());
    assert_eq!(b.checkpoints(), a.checkpoints());
    assert_eq!(
        b.asset_bytes("assets/b.png").unwrap().as_deref(),
        Some(&b"second image"[..])
    );

    // The expert code counts as settings.
    a.set_custom_text("#set text(size: 12pt)").unwrap();
    a.sync_push(&mut ra).unwrap();
    let p = b.sync_pull(&mut rb).unwrap();
    assert!(p.changed && p.settings && p.changes.is_empty());
    assert_eq!(b.custom_text(), a.custom_text());
}

#[test]
fn concurrent_edits_converge() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut a, mut ra, code) = setup(tmp.path());
    let (mut b, mut rb) = join(tmp.path(), 2, &code);
    a.insert(0, "A1 ").unwrap();
    b.insert(b.len(), "B1 \u{1F680}").unwrap();
    b.map_update(MapName::Meta, &set(&[("author", json!("B"))]))
        .unwrap();
    a.sync_push(&mut ra).unwrap();
    b.sync_push(&mut rb).unwrap();
    let (old_a, old_b) = (a.text(), b.text());
    let pa = a.sync_pull(&mut ra).unwrap();
    let pb = b.sync_pull(&mut rb).unwrap();
    assert_eq!(a.text(), b.text());
    assert!(a.text().starts_with("A1 ") && a.text().ends_with("B1 \u{1F680}"));
    assert_eq!(apply_cm(&old_a, &pa.changes), a.text());
    assert_eq!(apply_cm(&old_b, &pb.changes), b.text());
    assert!(pa.settings && !pb.settings);
    assert_eq!(a.map_entries(MapName::Meta), b.map_entries(MapName::Meta));
}

/// [`FsStorage`] counting reads.
struct Counting {
    inner: FsStorage,
    reads: Arc<AtomicUsize>,
}

impl Storage for Counting {
    fn location(&self) -> PathBuf {
        self.inner.location()
    }
    fn read(&self, path: &str) -> lethora_core::Result<Option<Vec<u8>>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.read(path)
    }
    fn read_from(&self, path: &str, offset: u64) -> lethora_core::Result<Option<Vec<u8>>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.read_from(path, offset)
    }
    fn size(&self, path: &str) -> lethora_core::Result<Option<u64>> {
        self.inner.size(path)
    }
    fn write_atomic(&self, path: &str, bytes: &[u8]) -> lethora_core::Result<()> {
        self.inner.write_atomic(path, bytes)
    }
    fn list(&self, dir: &str) -> lethora_core::Result<Vec<String>> {
        self.inner.list(dir)
    }
    fn create_dir(&self, dir: &str) -> lethora_core::Result<()> {
        self.inner.create_dir(dir)
    }
    fn rename(&self, from: &str, to: &str) -> lethora_core::Result<()> {
        self.inner.rename(from, to)
    }
    fn open_log(&self, path: &str) -> lethora_core::Result<Box<dyn LogFile>> {
        self.inner.open_log(path)
    }
    fn lock(&self, path: &str) -> lethora_core::Result<LockGuard> {
        self.inner.lock(path)
    }
}

#[test]
fn pull_without_changes_reads_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut a, mut ra, code) = setup(tmp.path());
    let reads = Arc::new(AtomicUsize::new(0));
    let counting = Counting {
        inner: FsStorage::new(sync_dir(tmp.path())),
        reads: reads.clone(),
    };
    let (mut b, mut rb) = Document::sync_join(
        fs_storage(&tmp.path().join("dev2")),
        Box::new(counting),
        &code,
        &who(2),
    )
    .unwrap();
    // B's first push copies everything; then nothing new is nothing written.
    b.sync_push(&mut rb).unwrap();
    let log = file(&sync_dir(tmp.path()), 2, "log");
    let len = fs::metadata(&log).unwrap().len();
    assert!(len > 0);
    b.sync_push(&mut rb).unwrap();
    assert_eq!(fs::metadata(&log).unwrap().len(), len);

    reads.store(0, Ordering::SeqCst);
    assert_eq!(b.sync_pull(&mut rb).unwrap(), Pulled::default());
    assert_eq!(reads.load(Ordering::SeqCst), 0);

    a.insert(0, "new ").unwrap();
    a.sync_push(&mut ra).unwrap();
    // A also reads B's first push: everything it knew, nothing new for A.
    let p = a.sync_pull(&mut ra).unwrap();
    assert!(!p.changed);
    let p = b.sync_pull(&mut rb).unwrap();
    assert!(p.changed);
    assert!(reads.load(Ordering::SeqCst) > 0);
    assert_eq!(b.text(), a.text());

    reads.store(0, Ordering::SeqCst);
    assert_eq!(b.sync_pull(&mut rb).unwrap(), Pulled::default());
    assert_eq!(reads.load(Ordering::SeqCst), 0);
}

#[test]
fn torn_tail_is_retried() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut a, mut ra, code) = setup(tmp.path());
    let (mut b, mut rb) = join(tmp.path(), 2, &code);
    a.insert(0, "one ").unwrap();
    a.sync_push(&mut ra).unwrap();
    assert!(b.sync_pull(&mut rb).unwrap().changed);
    a.insert(0, "two ").unwrap();
    a.sync_push(&mut ra).unwrap();

    let log = file(&sync_dir(tmp.path()), 1, "log");
    let full = fs::read(&log).unwrap();
    fs::write(&log, &full[..full.len() - 5]).unwrap();
    let p = b.sync_pull(&mut rb).unwrap();
    assert!(!p.changed && p.skipped.is_empty());
    assert!(b.text().starts_with("one "));

    // The rest arrives.
    fs::write(&log, &full).unwrap();
    let p = b.sync_pull(&mut rb).unwrap();
    assert!(p.changed && p.skipped.is_empty());
    assert_eq!(b.text(), a.text());
}

#[test]
fn compaction_is_detected() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut a, mut ra, code) = setup(tmp.path());
    let (mut b, mut rb) = join(tmp.path(), 2, &code);
    let sync = sync_dir(tmp.path());
    let log = file(&sync, 1, "log");

    // Compacted right after B read: the log shrank.
    a.insert(0, "1 ").unwrap();
    a.sync_push(&mut ra).unwrap();
    b.sync_pull(&mut rb).unwrap();
    a.set_compaction_threshold(0);
    a.insert(0, "2 ").unwrap();
    a.sync_push(&mut ra).unwrap();
    assert_eq!(fs::metadata(&log).unwrap().len(), 0);
    let p = b.sync_pull(&mut rb).unwrap();
    assert!(p.changed && p.skipped.is_empty());
    assert_eq!(b.text(), a.text());

    // Compacted and grown past where B was, before B looked again.
    a.set_compaction_threshold(u64::MAX);
    for i in 3..5 {
        a.insert(0, &format!("{i} ")).unwrap();
        a.sync_push(&mut ra).unwrap();
    }
    b.sync_pull(&mut rb).unwrap();
    let read_up_to = fs::metadata(&log).unwrap().len();
    a.set_compaction_threshold(0);
    a.insert(0, "5 ").unwrap();
    a.sync_push(&mut ra).unwrap();
    a.set_compaction_threshold(u64::MAX);
    let mut i = 6;
    while fs::metadata(&log).unwrap().len() <= read_up_to {
        a.insert(0, &format!("{i} ")).unwrap();
        a.sync_push(&mut ra).unwrap();
        i += 1;
    }
    let p = b.sync_pull(&mut rb).unwrap();
    assert!(p.changed && p.skipped.is_empty());
    assert_eq!(b.text(), a.text());
}

#[test]
fn tampered_record_is_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut a, mut ra, code) = setup(tmp.path());
    let (mut b, mut rb) = join(tmp.path(), 2, &code);
    let (mut c, mut rc) = join(tmp.path(), 3, &code);
    a.insert(0, "from A ").unwrap();
    a.sync_push(&mut ra).unwrap();
    c.insert(0, "from C ").unwrap();
    c.sync_push(&mut rc).unwrap();

    // One flipped bit in A's record, with a matching crc.
    let log = file(&sync_dir(tmp.path()), 1, "log");
    let bytes = fs::read(&log).unwrap();
    let mut record = records(&bytes).last().unwrap().to_vec();
    let last = record.len() - 20;
    record[last] ^= 1;
    let start = bytes.len() - record.len() - 8;
    let mut tampered = bytes[..start].to_vec();
    tampered.extend_from_slice(&frame(&record));
    fs::write(&log, &tampered).unwrap();

    let p = b.sync_pull(&mut rb).unwrap();
    assert!(p.changed);
    assert!(b.text().starts_with("from C "));
    assert!(!b.text().contains("from A"));
    assert_eq!(p.skipped.len(), 1);
    assert!(
        p.skipped[0].contains(who(1).device.as_str()),
        "{:?}",
        p.skipped
    );
    // Reported once; tried again only when A's files change.
    assert_eq!(b.sync_pull(&mut rb).unwrap(), Pulled::default());
}

#[test]
fn tampered_blob_is_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut a, mut ra, code) = setup(tmp.path());
    let (mut b, mut rb) = join(tmp.path(), 2, &code);
    a.put_asset("assets/b.png", b"second image").unwrap();
    a.sync_push(&mut ra).unwrap();
    let blob = a.asset_blob("assets/b.png").unwrap();
    let path = sync_dir(tmp.path()).join("assets").join(&blob);
    let mut bytes = fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    fs::write(&path, &bytes).unwrap();

    let p = b.sync_pull(&mut rb).unwrap();
    assert!(p.assets);
    assert_eq!(p.blobs, 0);
    assert_eq!(p.skipped.len(), 1);
    assert!(p.skipped[0].contains(&blob));
    assert_eq!(b.asset_bytes("assets/b.png").unwrap(), None);
    assert!(b.sync_pull(&mut rb).unwrap().skipped.is_empty());

    // A blob renamed to another blob's name doesn't open either.
    let other = a.asset_blob("assets/a.png").unwrap();
    let (mut c, mut rc) = {
        let sealed_a = fs::read(sync_dir(tmp.path()).join("assets").join(&other)).unwrap();
        fs::write(&path, sealed_a).unwrap();
        join(tmp.path(), 3, &code)
    };
    assert_eq!(c.asset_bytes("assets/b.png").unwrap(), None);
    assert_eq!(c.asset_bytes("assets/a.png").unwrap().as_deref(), Some(PNG));
    let p = c.sync_pull(&mut rc).unwrap();
    assert_eq!(p.blobs, 0);
}

#[test]
fn record_copied_to_another_device_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut a, mut ra, code) = setup(tmp.path());
    let (mut b, mut rb) = join(tmp.path(), 2, &code);
    a.insert(0, "secret ").unwrap();
    a.sync_push(&mut ra).unwrap();

    // A's record shows up as device 4's, and A's own log is emptied again.
    let sync = sync_dir(tmp.path());
    fs::copy(file(&sync, 1, "log"), file(&sync, 4, "log")).unwrap();
    fs::write(file(&sync, 1, "log"), b"").unwrap();
    let p = b.sync_pull(&mut rb).unwrap();
    assert!(!p.changed);
    assert!(!b.text().contains("secret"));
    assert_eq!(p.skipped.len(), 1);
    assert!(p.skipped[0].contains(who(4).device.as_str()));
}

#[test]
fn join_and_open_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, _ra, code) = setup(tmp.path());
    let sync = sync_dir(tmp.path());
    let dev2 = tmp.path().join("dev2");

    let wrong = ProjectCode::from_key([9; 32]);
    let err = Document::sync_join(fs_storage(&dev2), fs_storage(&sync), &wrong, &who(2));
    assert!(matches!(err, Err(Error::WrongCode(_))));
    assert!(!dev2.join("project.json").exists());
    assert!(matches!(
        a.sync_open(fs_storage(&sync), &wrong),
        Err(Error::WrongCode(_))
    ));

    let empty = tmp.path().join("empty");
    let err = Document::sync_join(fs_storage(&dev2), fs_storage(&empty), &code, &who(2));
    assert!(matches!(err, Err(Error::NotFound(_))));

    let newer = tmp.path().join("newer");
    fs::create_dir_all(&newer).unwrap();
    for json in [r#"{"format": 2, "project": "x"}"#, "not json"] {
        fs::write(newer.join("replica.json"), json).unwrap();
        let err = Document::sync_join(fs_storage(&dev2), fs_storage(&newer), &code, &who(2));
        assert!(matches!(err, Err(Error::UnsupportedFormat(_))));
    }

    // Into an existing project.
    let err = Document::sync_join(
        fs_storage(&tmp.path().join("a")),
        fs_storage(&sync),
        &code,
        &who(2),
    );
    assert!(matches!(err, Err(Error::AlreadyExists(_))));
}

/// Every file below `dir` with its contents.
fn tree(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = all_files(dir);
    files.sort();
    files
        .into_iter()
        .map(|f| {
            let bytes = fs::read(&f).unwrap();
            (f, bytes)
        })
        .collect()
}

#[test]
fn replica_project_checks_the_code_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (_a, replica, code) = setup(tmp.path());
    let sync = sync_dir(tmp.path());
    let project = replica.project_id().to_owned();
    drop(replica);
    // Without `main/` a write would show.
    fs::remove_dir_all(sync.join("main")).unwrap();
    let before = tree(&sync);

    let storage = FsStorage::new(&sync);
    assert_eq!(replica_project(&storage, &code).unwrap(), Some(project));
    let wrong = ProjectCode::from_key([9; 32]);
    assert_eq!(replica_project(&storage, &wrong).unwrap(), None);
    assert_eq!(tree(&sync), before);
    assert!(!sync.join("main").exists());

    let empty = tmp.path().join("empty");
    let err = replica_project(&FsStorage::new(&empty), &code);
    assert!(matches!(err, Err(Error::NotFound(_))));
    assert!(!empty.exists());
    let newer = tmp.path().join("newer");
    fs::create_dir_all(&newer).unwrap();
    fs::write(
        newer.join("replica.json"),
        r#"{"format": 2, "project": "x"}"#,
    )
    .unwrap();
    let err = replica_project(&FsStorage::new(&newer), &code);
    assert!(matches!(err, Err(Error::UnsupportedFormat(_))));
}

#[test]
fn stored_sync_settings_are_read_without_opening() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("a");
    let mut a = Document::create(&dir, &who(1)).unwrap();
    let storage = FsStorage::new(&dir);
    assert_eq!(stored_sync_settings(&storage, &who(1).device), None);
    let settings = SyncSettings {
        folder: tmp.path().join("cloud"),
        project: "0123456789abcdef0123456789abcdef".into(),
        level: None,
    };
    a.set_sync_settings(Some(settings.clone())).unwrap();
    // While the project is open, and for its own device only.
    assert_eq!(
        stored_sync_settings(&storage, &who(1).device),
        Some(settings)
    );
    assert_eq!(stored_sync_settings(&storage, &who(2).device), None);
}

#[test]
fn setup_refuses_an_existing_replica() {
    let tmp = tempfile::tempdir().unwrap();
    let (_a, _ra, _code) = setup(tmp.path());
    let mut c = Document::create(&tmp.path().join("c"), &who(3)).unwrap();
    assert!(matches!(
        c.sync_setup(fs_storage(&sync_dir(tmp.path()))),
        Err(Error::AlreadyExists(_))
    ));
}

#[test]
fn undo_after_pull_undoes_only_local_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut a, mut ra, code) = setup(tmp.path());
    let (mut b, mut rb) = join(tmp.path(), 2, &code);
    assert!(!b.can_undo());
    b.insert(0, "mine ").unwrap();
    a.insert(a.len(), "theirs").unwrap();
    a.sync_push(&mut ra).unwrap();
    b.sync_pull(&mut rb).unwrap();
    assert_eq!(b.text(), format!("mine {}", a.text()));
    let undone = b.undo().unwrap().unwrap();
    assert_eq!(undone, vec![ch(0, 5, "")]);
    assert_eq!(b.text(), a.text());
    assert!(!b.can_undo());
    assert!(b.redo().unwrap().is_some());
    assert_eq!(b.text(), format!("mine {}", a.text()));
}

#[test]
fn reopened_replica_pushes_only_new_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut a, mut ra, code) = setup(tmp.path());
    a.insert(0, "before ").unwrap();
    a.sync_push(&mut ra).unwrap();
    let sync = sync_dir(tmp.path());
    let settings = SyncSettings {
        folder: sync.clone(),
        project: ra.project_id().to_owned(),
        level: None,
    };
    a.set_sync_settings(Some(settings.clone())).unwrap();
    drop(ra);
    drop(a);

    let dir = tmp.path().join("a");
    let mut a = Document::open(&dir, &who(1)).unwrap();
    assert_eq!(a.sync_settings(), Some(&settings));
    // The mirror keeps the sync settings in the state file.
    a.set_bibliography("@book{c, title={C}}\n").unwrap();
    a.autosave().unwrap();
    let key = *ProjectCode::parse(&code.to_string()).unwrap().key();
    let mut ra = a
        .sync_open(fs_storage(&sync), &ProjectCode::from_key(key))
        .unwrap();
    assert_eq!(ra.project_id(), settings.project);
    let log = file(&sync, 1, "log");
    let len = fs::metadata(&log).unwrap().len();
    a.sync_push(&mut ra).unwrap();
    let grown = fs::metadata(&log).unwrap().len();
    assert!(grown > len, "the bibliography change is new");
    a.sync_push(&mut ra).unwrap();
    assert_eq!(fs::metadata(&log).unwrap().len(), grown);
    drop(a);
    let a = Document::open(&dir, &who(1)).unwrap();
    assert_eq!(a.sync_settings(), Some(&settings));

    let (b, _rb) = join(tmp.path(), 2, &code);
    assert_eq!(b.text(), a.text());
    assert_eq!(b.bibliography(), a.bibliography());
}

#[test]
fn pull_local_picks_up_live_edits() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut a = Document::create(dir, &who(1)).unwrap();
    a.insert(0, "start \u{1F600}").unwrap();
    a.autosave().unwrap();
    let mut b = Document::open(dir, &who(2)).unwrap();
    assert_eq!(b.text(), a.text());
    // Opening read A's files already.
    assert_eq!(b.pull_local().unwrap(), Pulled::default());
    assert_eq!(a.pull_local().unwrap(), Pulled::default());

    a.insert(0, "live ").unwrap();
    a.map_update(MapName::Layout, &set(&[("columns", json!(2))]))
        .unwrap();
    a.autosave().unwrap();
    let old = b.text();
    let p = b.pull_local().unwrap();
    assert!(p.changed && p.settings && p.skipped.is_empty());
    assert_eq!(apply_cm(&old, &p.changes), b.text());
    assert_eq!(b.text(), a.text());

    b.insert(b.len(), " back").unwrap();
    b.autosave().unwrap();
    let old = a.text();
    let p = a.pull_local().unwrap();
    assert_eq!(apply_cm(&old, &p.changes), a.text());
    assert_eq!(a.text(), b.text());
    assert_eq!(b.pull_local().unwrap(), Pulled::default());
}

/// `review/<device>.<ext>` in `dir`.
fn review_file(dir: &Path, n: u8, ext: &str) -> PathBuf {
    dir.join("review")
        .join(format!("{}.{ext}", who(n).device.as_str()))
}

#[test]
fn review_items_travel_through_the_replica_both_ways() {
    let tmp = tempfile::tempdir().unwrap();
    let mut a = Document::create(&tmp.path().join("a"), &who(1)).unwrap();
    a.insert(0, "Hello world.").unwrap();
    let first = a.add_comment(0, 5, "Before the setup.").unwrap();
    let (mut ra, code) = a.sync_setup(fs_storage(&sync_dir(tmp.path()))).unwrap();
    let sync = sync_dir(tmp.path());
    assert!(review_file(&sync, 1, "snapshot").is_file());

    // B joins and finds A's comment, with its author.
    let (mut b, mut rb) = join(tmp.path(), 2, &code);
    let items = b.review_items();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].messages[0].text, "Before the setup.");
    assert_eq!(items[0].author.as_ref().unwrap().user, who(1).user);

    // B answers and marks; A pulls both.
    b.reply(first, "B answers.").unwrap();
    b.add_mark(6, 11, "green").unwrap();
    b.sync_push(&mut rb).unwrap();
    assert!(review_file(&sync, 2, "log").is_file());
    let p = a.sync_pull(&mut ra).unwrap();
    assert!(p.review && !p.changed && p.skipped.is_empty(), "{p:?}");
    let items = a.review_items();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].messages[1].text, "B answers.");
    assert_eq!(
        items[0].messages[1].author.as_ref().unwrap().user,
        who(2).user
    );
    assert_eq!(items[1].colour.as_deref(), Some("green"));
    // The pull isn't an undo step of A's review.
    assert!(a.review_undo().unwrap());
    assert_eq!(a.review_items().len(), 1);
    assert!(a.review_redo().unwrap());

    // And back: A resolves, B sees it.
    a.resolve(first).unwrap();
    a.sync_push(&mut ra).unwrap();
    let p = b.sync_pull(&mut rb).unwrap();
    assert!(p.review, "{p:?}");
    assert!(b.review_items().iter().any(|i| i.id == first && i.resolved));
    // Nothing new: nothing read.
    assert_eq!(b.sync_pull(&mut rb).unwrap(), Pulled::default());
}

#[test]
fn a_replica_without_review_still_works() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut a, ra, code) = setup(tmp.path());
    let sync = sync_dir(tmp.path());
    // As a replica made before `review` was synced.
    drop(ra);
    fs::remove_dir_all(sync.join("review")).unwrap();
    let (mut b, mut rb) = join(tmp.path(), 2, &code);
    assert_eq!(b.text(), a.text());
    assert!(b.review_items().is_empty());
    let mut ra = a.sync_open(fs_storage(&sync), &code).unwrap();
    a.insert(0, "more ").unwrap();
    a.add_comment(0, 4, "Now with review.").unwrap();
    a.sync_push(&mut ra).unwrap();
    let p = b.sync_pull(&mut rb).unwrap();
    assert!(p.changed && p.review && p.skipped.is_empty(), "{p:?}");
    assert_eq!(b.text(), a.text());
    assert_eq!(b.review_items().len(), 1);
}

#[test]
fn main_and_review_files_dont_swap() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut a, mut ra, code) = setup(tmp.path());
    let (mut b, mut rb) = join(tmp.path(), 2, &code);
    let sync = sync_dir(tmp.path());
    a.add_comment(0, 1, "A review record.").unwrap();
    a.sync_push(&mut ra).unwrap();
    // A's review record copied into its main log, and its review log
    // emptied: neither document takes it.
    let record = fs::read(review_file(&sync, 1, "log")).unwrap();
    assert!(!record.is_empty());
    let mut main = fs::read(file(&sync, 1, "log")).unwrap();
    main.extend_from_slice(&record);
    fs::write(file(&sync, 1, "log"), &main).unwrap();
    fs::write(review_file(&sync, 1, "log"), b"").unwrap();
    let p = b.sync_pull(&mut rb).unwrap();
    assert!(!p.review && !p.changed, "{p:?}");
    assert_eq!(p.skipped.len(), 1);
    assert!(p.skipped[0].contains(who(1).device.as_str()));
    assert!(b.review_items().is_empty());

    // A main snapshot posing as a review snapshot doesn't open either.
    fs::copy(
        file(&sync, 1, "snapshot"),
        review_file(&sync, 1, "snapshot"),
    )
    .unwrap();
    let (c, _rc) = join(tmp.path(), 3, &code);
    assert!(c.review_items().is_empty());
    assert_eq!(c.text(), a.text());
}

/// The level of the last record of `log` (an envelope version 3).
fn last_level(log: &Path) -> u32 {
    let bytes = fs::read(log).unwrap();
    let last = *records(&bytes).last().unwrap();
    assert_eq!(last[0], 3, "envelope version");
    u32::from_le_bytes(last[5..9].try_into().unwrap())
}

#[test]
fn a_newer_record_stops_reading_its_device_before_it() {
    let tmp = tempfile::tempdir().unwrap();
    let sync = sync_dir(tmp.path());
    let (mut a, mut ra, code) = setup(tmp.path());
    let (mut b, mut rb) = join(tmp.path(), 2, &code);
    let settings = SyncSettings {
        folder: sync.clone(),
        project: ra.project_id().to_owned(),
        level: None,
    };
    a.set_sync_settings(Some(settings)).unwrap();
    // A plays a newer version: it writes at the project's level, 1, until
    // it raises it.
    ra.set_reader_level(2);
    a.insert(0, "one ").unwrap();
    a.sync_push(&mut ra).unwrap();
    assert_eq!((ra.level(), last_level(&file(&sync, 1, "log"))), (1, 1));
    assert!(matches!(
        a.sync_require(&mut ra, 3),
        Err(Error::InvalidChange(_))
    ));
    a.sync_require(&mut ra, 2).unwrap();
    assert_eq!(ra.level(), 2);
    assert_eq!(a.sync_settings().unwrap().level, Some(2));
    a.insert(0, "two ").unwrap();
    a.sync_push(&mut ra).unwrap();
    assert_eq!(last_level(&file(&sync, 1, "log")), 2);

    // B (level 1) takes the record before it and stops there.
    let p = b.sync_pull(&mut rb).unwrap();
    assert!(p.changed && p.skipped.is_empty(), "{p:?}");
    assert!(b.text().starts_with("one = Title"), "{}", b.text());
    assert_eq!(
        rb.needs_update(),
        Some("This project uses features of a newer Lethora; update to keep syncing.")
    );
    // Its own changes still go out, at level 1; A reads them.
    b.insert(b.len(), "from B").unwrap();
    b.sync_push(&mut rb).unwrap();
    assert_eq!((rb.level(), last_level(&file(&sync, 2, "log"))), (1, 1));
    assert!(!b.sync_pull(&mut rb).unwrap().changed);
    assert!(rb.needs_update().is_some());
    assert!(a.sync_pull(&mut ra).unwrap().changed);
    assert!(a.text().ends_with("from B"));

    // After B's update it reads on from there, and writes at the level.
    drop(rb);
    let mut rb = b.sync_open(fs_storage(&sync), &code).unwrap();
    rb.set_reader_level(2);
    b.sync_pull(&mut rb).unwrap();
    assert_eq!(b.text(), a.text());
    assert_eq!((rb.needs_update(), rb.level()), (None, 2));

    // A version that doesn't know its own files' level never rewrites them.
    drop(ra);
    assert!(matches!(
        a.sync_open(fs_storage(&sync), &code),
        Err(Error::UnsupportedFormat(_))
    ));
}
