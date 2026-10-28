//! Review tools: comment threads, marks (a highlighter) and suggestions, kept
//! in the project's second Loro document, `review`, and anchored in `main`'s
//! content text. A separate document because write rights differ (see
//! [`Document::set_role`]): reviewers write `review`, never `main`.
//!
//! `review` is persisted like `main` (`review/<device>.snapshot` and `.log`,
//! see the crate docs), saved by [`Document::autosave`], opened with the
//! project and imported from the other devices' files by
//! [`Document::pull_local`] ([`Pulled::review`](crate::Pulled::review)).
//! Server sync carries it as a stream of its own and the folder-sync
//! replica as `review/` (see the `server` and `sync` modules). Checkpoints
//! and [`Document::restore`] cover `main` only and never touch it.
//!
//! # Format
//!
//! - `peers`: who each Loro peer of `review` is, exactly like `main`'s map
//!   (written right before a session's first local change, in a commit with
//!   the origin `"lethora:sys"`; entries untrusted).
//! - `items`: key = the item id ([`ReviewId`], in decimal; JS-safe, drawn
//!   like checkpoint ids), value = a Loro map:
//!   - `created`: i64 Unix seconds;
//!   - `from`, `to`: binary, encoded Loro cursors into `main`'s `content`
//!     (see "Anchors");
//!   - `resolved`: bool;
//!   - `colour` (optional): one of [`MARK_COLOURS`];
//!   - `messages`: a Loro list (created with the item, so concurrent first
//!     replies both land) of Loro maps `{"id": <i64, a ReviewId>, "created":
//!     <i64>, "body": <LoroText>, "edited": <i64, optional>, "deleted":
//!     <bool, optional>}`; a deleted message keeps its place with an empty
//!     body;
//!   - suggestions only: `insert` (string: the new text), `original`
//!     (string: the range's text when suggested) and `state` (`"open"`,
//!     `"accepted"` or `"rejected"`);
//!   - `removed` (optional): `true` once removed. Items are never deleted
//!     from the map, so undoing a removal brings back the very same map,
//!     and its author with it.
//!
//! Unknown keys are ignored; an item with unreadable cursors, or with no
//! messages, no suggestion and no valid colour, is left out of the listing.
//! The kind follows from the content ([`ReviewKind`]): a suggestion has a
//! `state`; otherwise an item with messages is a comment (it keeps a colour
//! it had as a mark); otherwise one with a colour is a mark.
//!
//! # Anchors
//!
//! Positions are UTF-16 offsets into `main`'s content text, like
//! [`TextChange`]. A range `from..to` stores two cursors: `from` on the
//! character at `from` ([`Side::Middle`]: the range starts where that
//! character is), `to` on the range's last character (at `to - 1`) with
//! [`Side::Right`]: the range ends right after it. So text inserted just
//! before the range lands before its first character and text inserted just
//! after it lands after its last one, both outside; text typed inside grows
//! it. An empty range (an insertion point, e.g. for an insert suggestion)
//! stores the same `Side::Middle` cursor twice: it stays before the
//! character it was in front of (at the end of the text: at the end).
//! When the first or last character is deleted, the cursor resolves to
//! where it was. A non-empty range whose characters are all gone resolves
//! to an empty range and is *detached*: still listed (at the place where
//! its text was), never dropped. A cursor whose character hasn't arrived
//! here yet (another device's `main` changes not imported) counts as
//! detached at 0. Cursors follow edits (other devices' too), undo/redo of
//! insertions and reopening like [`Document::anchor_at`]. Known limitation,
//! as for those anchors: undo brings deleted text back as new characters,
//! so after undoing the deletion of a range's first or last character the
//! range no longer covers it, and after undoing the deletion of all of it
//! the item stays detached (at the restored text).
//!
//! # Authors
//!
//! Who made an item or a message comes from Loro, never from a stored
//! field: the op that created its map (the peer and counter in the map's
//! container id, which no later edit changes), mapped through `review`'s
//! `peers` map to a user and device; a peer without a valid entry is
//! unknown. In a server-shared project the op must also have come from that
//! device's own stream (see the `trust` module): an entry or op forged by
//! another device leaves the item's author unknown, so it grants no
//! author's rights. "Author" in the rules below is the user: any of its
//! devices counts.
//!
//! # Permissions
//!
//! These are client rules, checked here: inside one document the server
//! can't tell one change from another, so it only enforces who writes
//! `review` at all. Every entry that changes `review` needs it writable
//! (role `None`, Owner, Editor or Reviewer; else [`Error::ReadOnly`]).
//! On top of that:
//!
//! | Entry | Who |
//! | --- | --- |
//! | add a comment, mark or suggestion, reply, resolve, reopen | any review writer |
//! | edit a message, recolour an item | its author |
//! | delete a message, remove an item, reject a suggestion | its author, an Owner or an Editor (role `None` counts as owner) |
//! | accept a suggestion | whoever writes `main` (Owner, Editor, `None`), never a Reviewer |
//!
//! Author failures are [`Error::NotAuthor`], role failures
//! [`Error::ReadOnly`]. Unknown ids are [`Error::NotFound`].
//!
//! # Suggest mode
//!
//! The app's track-changes mode calls [`Document::suggest_change`] for each
//! edit instead of changing `main`. An edit next to or inside this user's
//! own open suggestion (open, not resolved, not detached; the newest one if
//! several qualify) extends it instead of making a new one:
//!
//! 1. Typing (`from == to`, text) at the own suggestion's end, or inside its
//!    range, appends the text to its `insert`.
//! 2. Deleting (`from < to`, no text) up to the own suggestion's end while
//!    its `insert` holds at least as many characters (a backspace after
//!    typed text) removes that many characters from the end of `insert`
//!    instead; a suggestion whose `insert` and range are both empty then is
//!    removed.
//! 3. Otherwise, deleting or replacing over or right next to the own
//!    suggestion's range (touching it at either end) widens the range to
//!    cover both, extends `original` by the newly covered text and appends
//!    the replacement text, if any, to `insert`. Deleting inside the range
//!    changes nothing.
//!
//! `main` doesn't change, so positions stay those of `main`'s text; moving
//! the caret past struck text is the app's part. Consecutive suggestions
//! merge into one undo step like typing ([`crate::UNDO_MERGE_INTERVAL_MS`],
//! [`Document::end_undo_step`]).
//!
//! # Undo
//!
//! `review` has its own undo history ([`Document::review_undo`]); each entry
//! is one step of its own (suggestions merge as above), and bookkeeping
//! commits are ignored as in `main`. [`Document::accept_suggestion`] is a
//! `main` undo step (replacing the range with `insert`); the suggestion's
//! state `accepted` is written as bookkeeping, so it is not undoable. Known
//! limitation: undoing the accepted change in `main` doesn't reopen the
//! suggestion.

use std::collections::BTreeSet;
use std::time::SystemTime;

use loro::cursor::{Cursor, PosType, Side};
use loro::{
    Container, ContainerID, ContainerTrait, LoroDoc, LoroList, LoroMap, LoroText, LoroValue,
    PeerID, UndoManager, ValueOrContainer,
};
use serde::{Deserialize, Serialize};

use crate::trust::Trust;
use crate::{
    DeviceId, Document, Error, Files, MAX_UNDO_STEPS, Part, REVIEW, Result, SYS_ORIGIN, TextChange,
    UNDO_MERGE_INTERVAL_MS, UserId, new_loro_doc, register, sync, unix_secs,
};

const ITEMS: &str = "items";
const CREATED: &str = "created";
const FROM: &str = "from";
const TO: &str = "to";
const RESOLVED: &str = "resolved";
const COLOUR: &str = "colour";
const MESSAGES: &str = "messages";
const INSERT: &str = "insert";
const ORIGINAL: &str = "original";
const STATE: &str = "state";
const REMOVED: &str = "removed";
const ID: &str = "id";
const BODY: &str = "body";
const EDITED: &str = "edited";
const DELETED: &str = "deleted";

/// The highlight colours of marks (and comments made from them).
pub const MARK_COLOURS: [&str; 4] = ["yellow", "green", "blue", "pink"];

/// Identifier of a review item or message, unique within a project; below
/// 2^53 and never 0, like a [`CheckpointId`](crate::CheckpointId).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReviewId(pub u64);

/// What a review item is (see the module docs, "Format").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewKind {
    /// A highlight: a colour, no messages.
    Mark,
    /// A thread of messages (maybe with a colour).
    Comment,
    /// A proposed change of the range's text (maybe with messages).
    Suggestion,
}

/// Where a suggestion stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SuggestionState {
    Open,
    Accepted,
    Rejected,
}

impl SuggestionState {
    fn as_str(self) -> &'static str {
        match self {
            SuggestionState::Open => "open",
            SuggestionState::Accepted => "accepted",
            SuggestionState::Rejected => "rejected",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        [Self::Open, Self::Accepted, Self::Rejected]
            .into_iter()
            .find(|state| state.as_str() == s)
    }
}

/// Who made a review item or message (see the module docs, "Authors").
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewAuthor {
    pub user: UserId,
    pub device: DeviceId,
}

/// A message of a review item's thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewMessage {
    pub id: ReviewId,
    /// `None` for an unknown peer.
    pub author: Option<ReviewAuthor>,
    /// Empty once deleted.
    pub text: String,
    /// Unix seconds.
    pub created: i64,
    /// Unix seconds of the last edit, if any.
    pub edited: Option<i64>,
    pub deleted: bool,
}

/// The suggestion part of a review item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Suggestion {
    /// The text that replaces the range (empty: a deletion).
    pub insert: String,
    /// The range's text when suggested (empty: an insertion).
    pub original: String,
    pub state: SuggestionState,
    /// The range's text now differs from `original` (or it is detached);
    /// such a suggestion can't be accepted.
    pub stale: bool,
}

/// A review item as listed by [`Document::review_items`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewItem {
    pub id: ReviewId,
    pub kind: ReviewKind,
    /// The range now, UTF-16 offsets into the content text; empty (where
    /// the text was) when detached.
    pub from: usize,
    pub to: usize,
    /// All of the range's text was deleted (see the module docs, "Anchors").
    pub detached: bool,
    /// One of [`MARK_COLOURS`], if highlighted.
    pub colour: Option<String>,
    pub resolved: bool,
    /// `None` for an unknown peer.
    pub author: Option<ReviewAuthor>,
    /// Unix seconds.
    pub created: i64,
    /// Oldest first.
    pub messages: Vec<ReviewMessage>,
    pub suggestion: Option<Suggestion>,
}

/// The `review` document of an open project with its files, undo history
/// and peer registration (the counterparts of `main`'s in [`Document`]).
pub(crate) struct Review {
    pub(crate) doc: LoroDoc,
    undo: UndoManager,
    /// This device's files of `review`.
    pub(crate) files: Files,
    /// What has been read of the other devices' `review/` files.
    pub(crate) reads: sync::Reads,
    /// The Loro peer whose `peers` entry is known to be written.
    pub(crate) registered: Option<PeerID>,
    /// The next suggestion starts an undo step of its own.
    pub(crate) split_next_edit: bool,
    /// Where its peers' ops came from (see the `trust` module).
    pub(crate) trust: Trust,
}

impl Review {
    /// `doc` loaded from `files`, with `reads` of the other devices.
    pub(crate) fn new(doc: LoroDoc, files: Files, reads: sync::Reads) -> Self {
        let mut undo = UndoManager::new(&doc);
        undo.set_merge_interval(UNDO_MERGE_INTERVAL_MS);
        undo.set_max_undo_steps(MAX_UNDO_STEPS);
        undo.add_exclude_origin_prefix(SYS_ORIGIN);
        Self {
            doc,
            undo,
            files,
            reads,
            registered: None,
            split_next_edit: false,
            trust: Trust::default(),
        }
    }

    /// An empty `review` without files yet (a new project).
    pub(crate) fn empty() -> Self {
        let doc = new_loro_doc();
        let files = Files {
            dir: REVIEW,
            log: None,
            saved_vv: doc.oplog_vv(),
        };
        Self::new(doc, files, sync::Reads::new())
    }
}

/// Where a stored cursor is now (see [`Document::locate`]).
struct Located {
    /// Unicode scalar offset; `None` if its character hasn't arrived.
    at: Option<usize>,
    /// The cursor marks the end of a non-empty range.
    after: bool,
}

/// An own open suggestion's current state, for merging.
struct Own {
    id: ReviewId,
    map: LoroMap,
    from: usize,
    to: usize,
    insert: String,
    original: String,
}

impl Document {
    /// The review items, in text order (detached ones where their text was),
    /// removed ones left out (see the module docs).
    pub fn review_items(&self) -> Vec<ReviewItem> {
        let mut items = Vec::new();
        self.items_map().for_each(|key, value| {
            if let ValueOrContainer::Container(Container::Map(map)) = value
                && let Some(item) = self.parse_item(key, &map)
            {
                items.push(item);
            }
        });
        items.sort_by_key(|i| (i.from, i.to, i.created, i.id));
        items
    }

    /// Adds a comment thread on UTF-16 range `from..to` (may be empty) with
    /// a first message `text`, as one review undo step. Fails with
    /// [`Error::InvalidChange`] on a range outside the text or splitting a
    /// surrogate pair.
    pub fn add_comment(&mut self, from: usize, to: usize, text: &str) -> Result<ReviewId> {
        self.check_review_writable()?;
        let (start, end) = self.range_cursors(from, to)?;
        self.register_review_peer()?;
        let (id, map) = self.new_item(start, end)?;
        self.push_message(&map, text)?;
        self.commit_review(false);
        Ok(id)
    }

    /// Adds message `text` to item `item`'s thread (a mark becomes a
    /// comment) as one review undo step; returns the message's id.
    pub fn reply(&mut self, item: ReviewId, text: &str) -> Result<ReviewId> {
        self.check_review_writable()?;
        let (map, _) = self.item(item)?;
        self.register_review_peer()?;
        let id = self.push_message(&map, text)?;
        self.commit_review(false);
        Ok(id)
    }

    /// Replaces the text of message `message` of item `item` with `text`
    /// and records the edit time, as one review undo step. Its author only;
    /// a deleted message can't be edited ([`Error::InvalidChange`]).
    pub fn edit_message(&mut self, item: ReviewId, message: ReviewId, text: &str) -> Result<()> {
        self.check_review_writable()?;
        let (map, found) = self.message(item, message)?;
        self.check_author(&found.author, "edit this message", false)?;
        if found.deleted {
            return Err(Error::InvalidChange("the message is deleted".into()));
        }
        if found.text == text {
            return Ok(());
        }
        self.register_review_peer()?;
        let body = body_of(&map)?;
        body.delete(0, body.len_unicode())?;
        body.insert(0, text)?;
        map.insert(EDITED, now())?;
        self.commit_review(false);
        Ok(())
    }

    /// Deletes message `message` of item `item` (it stays in the thread,
    /// empty and marked deleted) as one review undo step. Its author, an
    /// Owner or an Editor.
    pub fn delete_message(&mut self, item: ReviewId, message: ReviewId) -> Result<()> {
        self.check_review_writable()?;
        let (map, found) = self.message(item, message)?;
        self.check_author(&found.author, "delete this message", true)?;
        if found.deleted {
            return Ok(());
        }
        self.register_review_peer()?;
        let body = body_of(&map)?;
        body.delete(0, body.len_unicode())?;
        map.insert(DELETED, true)?;
        self.commit_review(false);
        Ok(())
    }

    /// Marks item `item` resolved, as one review undo step (nothing if it
    /// is already).
    pub fn resolve(&mut self, item: ReviewId) -> Result<()> {
        self.set_resolved(item, true)
    }

    /// Marks item `item` open again, as one review undo step (nothing if it
    /// is open).
    pub fn reopen(&mut self, item: ReviewId) -> Result<()> {
        self.set_resolved(item, false)
    }

    fn set_resolved(&mut self, item: ReviewId, resolved: bool) -> Result<()> {
        self.check_review_writable()?;
        let (map, found) = self.item(item)?;
        if found.resolved == resolved {
            return Ok(());
        }
        self.register_review_peer()?;
        map.insert(RESOLVED, resolved)?;
        self.commit_review(false);
        Ok(())
    }

    /// Adds a mark (a highlight) in `colour` (one of [`MARK_COLOURS`]) on
    /// the non-empty UTF-16 range `from..to`, as one review undo step.
    /// Fails with [`Error::InvalidChange`] on an unknown colour or a bad or
    /// empty range.
    pub fn add_mark(&mut self, from: usize, to: usize, colour: &str) -> Result<ReviewId> {
        self.check_review_writable()?;
        check_colour(colour)?;
        if from >= to {
            return Err(Error::InvalidChange("a mark needs some text".into()));
        }
        let (start, end) = self.range_cursors(from, to)?;
        self.register_review_peer()?;
        let (id, map) = self.new_item(start, end)?;
        map.insert(COLOUR, colour)?;
        self.commit_review(false);
        Ok(id)
    }

    /// Sets item `item`'s colour (one of [`MARK_COLOURS`]), as one review
    /// undo step. Its author only.
    pub fn set_mark_colour(&mut self, item: ReviewId, colour: &str) -> Result<()> {
        self.check_review_writable()?;
        check_colour(colour)?;
        let (map, found) = self.item(item)?;
        self.check_author(&found.author, "recolour this item", false)?;
        if found.colour.as_deref() == Some(colour) {
            return Ok(());
        }
        self.register_review_peer()?;
        map.insert(COLOUR, colour)?;
        self.commit_review(false);
        Ok(())
    }

    /// Removes item `item` (any kind), as one review undo step. Its author,
    /// an Owner or an Editor.
    pub fn remove_item(&mut self, item: ReviewId) -> Result<()> {
        self.check_review_writable()?;
        let (map, found) = self.item(item)?;
        self.check_author(&found.author, "remove this item", true)?;
        self.register_review_peer()?;
        map.insert(REMOVED, true)?;
        self.commit_review(false);
        Ok(())
    }

    /// Suggests replacing UTF-16 range `from..to` of the content text with
    /// `insert` (an empty range inserts, an empty `insert` deletes), merging
    /// into this user's own open suggestion next to it (see the module docs,
    /// "Suggest mode"). `main` doesn't change. Returns the suggestion made
    /// or extended, `None` if the edit removed it (or changed nothing: an
    /// empty range and text). Fails with [`Error::InvalidChange`] on a bad
    /// range.
    pub fn suggest_change(
        &mut self,
        from: usize,
        to: usize,
        insert: &str,
    ) -> Result<Option<ReviewId>> {
        self.check_review_writable()?;
        let (start, end) = self.range_cursors(from, to)?;
        if from == to && insert.is_empty() {
            return Ok(None);
        }
        let deleted = self.text.slice_utf16(from, to)?;
        let own = self.own_suggestion(from, to, insert, deleted.chars().count());
        self.register_review_peer()?;
        let Some(own) = own else {
            let (id, map) = self.new_item(start, end)?;
            map.insert(INSERT, insert)?;
            map.insert(ORIGINAL, deleted.as_str())?;
            map.insert(STATE, SuggestionState::Open.as_str())?;
            self.commit_review(true);
            return Ok(Some(id));
        };
        let mut result = Some(own.id);
        if from == to {
            // Rule 1: typing at its end or inside it.
            own.map.insert(INSERT, format!("{}{insert}", own.insert))?;
        } else if insert.is_empty()
            && to == own.to
            && own.insert.chars().count() >= deleted.chars().count()
        {
            // Rule 2: a backspace after typed text.
            let keep = own.insert.chars().count() - deleted.chars().count();
            let shrunk: String = own.insert.chars().take(keep).collect();
            if shrunk.is_empty() && own.from == own.to {
                own.map.insert(REMOVED, true)?;
                result = None;
            } else {
                own.map.insert(INSERT, shrunk)?;
            }
        } else {
            // Rule 3: widen the range over the deletion or replacement.
            let (lo, hi) = (from.min(own.from), to.max(own.to));
            if (lo, hi) != (own.from, own.to) {
                let original = format!(
                    "{}{}{}",
                    self.text.slice_utf16(lo, own.from)?,
                    own.original,
                    self.text.slice_utf16(own.to, hi)?
                );
                let (start, end) = self.range_cursors(lo, hi)?;
                own.map.insert(FROM, start)?;
                own.map.insert(TO, end)?;
                own.map.insert(ORIGINAL, original)?;
            }
            if !insert.is_empty() {
                own.map.insert(INSERT, format!("{}{insert}", own.insert))?;
            }
        }
        self.commit_review(true);
        Ok(result)
    }

    /// Accepts suggestion `item`: replaces its range in `main` with its
    /// `insert`, as one `main` undo step of its own, and marks it accepted
    /// (bookkeeping, not undoable; see the module docs, "Undo"). Returns
    /// what it changed in the text. Needs `main` writable (never a
    /// Reviewer). Fails with [`Error::InvalidChange`] if `item` is not an
    /// open suggestion and with [`Error::Stale`] if its text changed or was
    /// deleted.
    pub fn accept_suggestion(&mut self, item: ReviewId) -> Result<Vec<TextChange>> {
        self.check_writable()?;
        let (map, found) = self.item(item)?;
        let suggestion = open_suggestion(&found)?;
        if found.detached {
            return Err(Error::Stale("its text was deleted".into()));
        }
        if suggestion.stale {
            return Err(Error::Stale("the text under it changed".into()));
        }
        self.register_peer()?;
        self.register_review_peer()?;
        let (from, to, insert) = (found.from, found.to, suggestion.insert.clone());
        let ((), changes) = self.track(|d| {
            if from < to || !insert.is_empty() {
                d.text.splice_utf16(from, to - from, &insert)?;
            }
            d.split_next_edit = true;
            d.commit_edit(Part::Content);
            Ok(())
        })?;
        self.split_next_edit = true;
        self.review.doc.commit();
        map.insert(STATE, SuggestionState::Accepted.as_str())?;
        self.review.doc.set_next_commit_origin(SYS_ORIGIN);
        self.review.doc.commit();
        Ok(changes)
    }

    /// Rejects suggestion `item` (`main` stays as it is), as one review
    /// undo step. Its author, an Owner or an Editor; fails with
    /// [`Error::InvalidChange`] if `item` is not an open suggestion.
    pub fn reject_suggestion(&mut self, item: ReviewId) -> Result<()> {
        self.check_review_writable()?;
        let (map, found) = self.item(item)?;
        self.check_author(&found.author, "reject this suggestion", true)?;
        open_suggestion(&found)?;
        self.register_review_peer()?;
        map.insert(STATE, SuggestionState::Rejected.as_str())?;
        self.commit_review(false);
        Ok(())
    }

    /// Undoes the last step of `review`'s own history (`main` is left
    /// alone); returns whether there was one.
    pub fn review_undo(&mut self) -> Result<bool> {
        self.check_review_writable()?;
        if !self.can_review_undo() {
            return Ok(false);
        }
        self.register_review_peer()?;
        self.review.doc.commit();
        let done = self.review.undo.undo()?;
        self.review.split_next_edit = true;
        Ok(done)
    }

    /// Redoes the last undone step of `review`'s history; returns whether
    /// there was one.
    pub fn review_redo(&mut self) -> Result<bool> {
        self.check_review_writable()?;
        if !self.can_review_redo() {
            return Ok(false);
        }
        self.register_review_peer()?;
        self.review.doc.commit();
        let done = self.review.undo.redo()?;
        self.review.split_next_edit = true;
        Ok(done)
    }

    /// Whether `review` has a step to undo.
    pub fn can_review_undo(&self) -> bool {
        self.review.undo.can_undo()
    }

    /// Whether `review` has a step to redo.
    pub fn can_review_redo(&self) -> bool {
        self.review.undo.can_redo()
    }

    /// Fails with [`Error::ReadOnly`] while `review` is read only here.
    fn check_review_writable(&self) -> Result<()> {
        if self.is_review_writable() {
            Ok(())
        } else {
            Err(self.read_only())
        }
    }

    /// Writes this session's `peers` entry in `review` before its first
    /// local change there (see the crate docs, "Attribution").
    fn register_review_peer(&mut self) -> Result<()> {
        self.check_review_writable()?;
        register(
            &self.review.doc,
            &mut self.review.registered,
            &self.identity,
        )
    }

    /// Fails with [`Error::NotAuthor`] (`what` names the action) unless
    /// this user made the item or message, or `moderators` and this device
    /// writes `main` (an Owner or Editor).
    fn check_author(
        &self,
        author: &Option<ReviewAuthor>,
        what: &str,
        moderators: bool,
    ) -> Result<()> {
        let mine = author
            .as_ref()
            .is_some_and(|a| a.user == self.identity.user);
        if mine || (moderators && self.is_writable()) {
            Ok(())
        } else {
            Err(Error::NotAuthor(what.into()))
        }
    }

    /// Commits the pending `review` ops as an undo step of their own, or
    /// (`merge`, suggestions) merged with a directly preceding suggestion
    /// within the merge interval.
    fn commit_review(&mut self, merge: bool) {
        let review = &mut self.review;
        if merge && !review.split_next_edit {
            review.doc.commit();
        } else {
            review.undo.set_merge_interval(0);
            review.doc.commit();
            review.undo.set_merge_interval(UNDO_MERGE_INTERVAL_MS);
        }
        review.split_next_edit = !merge;
    }

    fn items_map(&self) -> LoroMap {
        self.review.doc.get_map(ITEMS)
    }

    /// Creates the map of a new item on the stored range `start..end`.
    fn new_item(&mut self, start: Vec<u8>, end: Vec<u8>) -> Result<(ReviewId, LoroMap)> {
        let items = self.items_map();
        let mut taken = BTreeSet::new();
        items.for_each(|key, _| {
            if let Ok(id) = key.parse() {
                taken.insert(id);
            }
        });
        let id = ReviewId(self.new_checkpoint_id(&taken).0);
        let map = items.insert_container(&id.0.to_string(), LoroMap::new())?;
        map.insert(CREATED, now())?;
        map.insert(FROM, start)?;
        map.insert(TO, end)?;
        map.insert(RESOLVED, false)?;
        map.insert_container(MESSAGES, LoroList::new())?;
        Ok((id, map))
    }

    /// Appends a message `text` to the thread of item map `item`.
    fn push_message(&mut self, item: &LoroMap, text: &str) -> Result<ReviewId> {
        let list = match item.get(MESSAGES) {
            Some(ValueOrContainer::Container(Container::List(list))) => list,
            // Created with every item; only foreign data lacks it.
            _ => item.insert_container(MESSAGES, LoroList::new())?,
        };
        let taken: BTreeSet<u64> = message_maps(&list)
            .iter()
            .filter_map(|m| int(m, ID).and_then(|id| u64::try_from(id).ok()))
            .collect();
        let id = ReviewId(self.new_checkpoint_id(&taken).0);
        let map = list.push_container(LoroMap::new())?;
        map.insert(ID, id.0 as i64)?;
        map.insert(CREATED, now())?;
        let body = map.insert_container(BODY, LoroText::new())?;
        body.insert(0, text)?;
        Ok(id)
    }

    /// Item `id`'s map and listing; [`Error::NotFound`] if there is no
    /// such item (or it is removed or invalid).
    fn item(&self, id: ReviewId) -> Result<(LoroMap, ReviewItem)> {
        let key = id.0.to_string();
        if let Some(ValueOrContainer::Container(Container::Map(map))) = self.items_map().get(&key)
            && let Some(item) = self.parse_item(&key, &map)
        {
            return Ok((map, item));
        }
        Err(Error::NotFound(format!("review item {}", id.0)))
    }

    /// Message `message` of item `item`: its map and listing.
    fn message(&self, item: ReviewId, message: ReviewId) -> Result<(LoroMap, ReviewMessage)> {
        let (map, _) = self.item(item)?;
        if let Some(ValueOrContainer::Container(Container::List(list))) = map.get(MESSAGES) {
            for m in message_maps(&list) {
                if let Some(found) = self.parse_message(&m)
                    && found.id == message
                {
                    return Ok((m, found));
                }
            }
        }
        Err(Error::NotFound(format!(
            "message {} of review item {}",
            message.0, item.0
        )))
    }

    /// The item stored under `key` in `map`, if valid and not removed.
    fn parse_item(&self, key: &str, map: &LoroMap) -> Option<ReviewItem> {
        let id = key.parse::<u64>().ok().filter(|id| id.to_string() == key)?;
        if flag(map, REMOVED) {
            return None;
        }
        let (Some(LoroValue::Binary(start)), Some(LoroValue::Binary(end))) =
            (value(map, FROM), value(map, TO))
        else {
            return None;
        };
        let (from, to, detached) = self.span(&start, &end)?;
        let messages = match map.get(MESSAGES) {
            Some(ValueOrContainer::Container(Container::List(list))) => message_maps(&list)
                .iter()
                .filter_map(|m| self.parse_message(m))
                .collect(),
            _ => Vec::new(),
        };
        let colour = string(map, COLOUR).filter(|c| MARK_COLOURS.contains(&c.as_str()));
        let suggestion = string(map, STATE)
            .and_then(|s| SuggestionState::parse(&s))
            .map(|state| {
                let original = string(map, ORIGINAL).unwrap_or_default();
                let now = self.text.slice_utf16(from, to).ok();
                Suggestion {
                    insert: string(map, INSERT).unwrap_or_default(),
                    stale: detached || now.as_ref() != Some(&original),
                    original,
                    state,
                }
            });
        let kind = match (&suggestion, messages.is_empty(), &colour) {
            (Some(_), _, _) => ReviewKind::Suggestion,
            (None, false, _) => ReviewKind::Comment,
            (None, true, Some(_)) => ReviewKind::Mark,
            (None, true, None) => return None,
        };
        Some(ReviewItem {
            id: ReviewId(id),
            kind,
            from,
            to,
            detached,
            colour,
            resolved: flag(map, RESOLVED),
            author: self.review_author(&map.id()),
            created: int(map, CREATED).unwrap_or(0),
            messages,
            suggestion,
        })
    }

    /// A message map's listing, if valid.
    fn parse_message(&self, map: &LoroMap) -> Option<ReviewMessage> {
        let id = u64::try_from(int(map, ID)?).ok()?;
        let text = match map.get(BODY) {
            Some(ValueOrContainer::Container(Container::Text(body))) => body.to_string(),
            _ => String::new(),
        };
        Some(ReviewMessage {
            id: ReviewId(id),
            author: self.review_author(&map.id()),
            text,
            created: int(map, CREATED).unwrap_or(0),
            edited: int(map, EDITED),
            deleted: flag(map, DELETED),
        })
    }

    /// The author of the container `id`: the peer that created it, through
    /// `review`'s `peers` map.
    fn review_author(&self, id: &ContainerID) -> Option<ReviewAuthor> {
        let ContainerID::Normal { peer, counter, .. } = id else {
            return None;
        };
        let (user, device) = self.peer_author(true, *peer).at(*counter).cloned()?;
        Some(ReviewAuthor { user, device })
    }

    /// The cursors storing UTF-16 range `from..to` (see the module docs,
    /// "Anchors"), encoded. Fails with [`Error::InvalidChange`] if the range
    /// is reversed, outside the text or splits a surrogate pair.
    fn range_cursors(&self, from: usize, to: usize) -> Result<(Vec<u8>, Vec<u8>)> {
        let valid = from <= to
            && to <= self.len_utf16()
            && self.is_utf16_boundary(from)
            && self.is_utf16_boundary(to);
        let unicode = |pos| self.text.convert_pos(pos, PosType::Utf16, PosType::Unicode);
        let (true, Some(start), Some(end)) = (valid, unicode(from), unicode(to)) else {
            return Err(bad_range(from, to));
        };
        let cursor = |pos, side| {
            self.text
                .get_cursor(pos, side)
                .map(|c| c.encode())
                .ok_or_else(|| bad_range(from, to))
        };
        let first = cursor(start, Side::Middle)?;
        let last = if end > start {
            cursor(end - 1, Side::Right)?
        } else {
            first.clone()
        };
        Ok((first, last))
    }

    /// Where a stored cursor is now; `None` if it is not a cursor into the
    /// content text.
    fn locate(&self, bytes: &[u8]) -> Option<Located> {
        let cursor = Cursor::decode(bytes).ok()?;
        if cursor.container != self.text.id() {
            return None;
        }
        let after = cursor.id.is_some() && cursor.side == Side::Right;
        let at = self.doc.get_cursor_pos(&cursor).ok().map(|found| {
            // Loro suggests an updated cursor only when the character is gone.
            let gone = cursor.id.is_some() && found.update.is_some();
            (found.current.pos + usize::from(after && !gone)).min(self.len())
        });
        Some(Located { at, after })
    }

    /// The stored range `start..end` now: UTF-16 offsets and whether it is
    /// detached; `None` for invalid cursors.
    fn span(&self, start: &[u8], end: &[u8]) -> Option<(usize, usize, bool)> {
        let (start, end) = (self.locate(start)?, self.locate(end)?);
        let (Some(mut from), Some(mut to)) = (start.at, end.at) else {
            return Some((0, 0, true));
        };
        let detached = end.after && from >= to;
        if from > to || detached {
            from = from.min(to);
            to = from;
        }
        let utf16 = |pos| self.text.convert_pos(pos, PosType::Unicode, PosType::Utf16);
        Some((utf16(from)?, utf16(to)?, detached))
    }

    /// This user's own open suggestion that the edit replacing UTF-16
    /// `from..to` (`deleted` characters) with `insert` merges into, by the
    /// rules in the module docs; the newest if several qualify.
    fn own_suggestion(&self, from: usize, to: usize, insert: &str, deleted: usize) -> Option<Own> {
        self.review_items()
            .into_iter()
            .filter(|i| {
                let mine = i.author.as_ref().map(|a| &a.user) == Some(&self.identity.user);
                let open = i
                    .suggestion
                    .as_ref()
                    .is_some_and(|s| s.state == SuggestionState::Open);
                mine && open && !i.resolved && !i.detached
            })
            .filter(|i| {
                let inserted = i
                    .suggestion
                    .as_ref()
                    .map_or(0, |s| s.insert.chars().count());
                if from == to {
                    from == i.to || (i.from < from && from <= i.to)
                } else {
                    let backspace = insert.is_empty() && to == i.to && inserted >= deleted;
                    backspace || (from <= i.to && to >= i.from)
                }
            })
            .max_by_key(|i| (i.created, i.id))
            .and_then(|i| {
                let (map, _) = self.item(i.id).ok()?;
                let s = i.suggestion?;
                Some(Own {
                    id: i.id,
                    map,
                    from: i.from,
                    to: i.to,
                    insert: s.insert,
                    original: s.original,
                })
            })
    }
}

/// The open suggestion part of `item`; [`Error::InvalidChange`] otherwise.
fn open_suggestion(item: &ReviewItem) -> Result<&Suggestion> {
    match &item.suggestion {
        Some(s) if s.state == SuggestionState::Open => Ok(s),
        Some(_) => Err(Error::InvalidChange(
            "the suggestion was accepted or rejected already".into(),
        )),
        None => Err(Error::InvalidChange(format!(
            "review item {} is not a suggestion",
            item.id.0
        ))),
    }
}

fn check_colour(colour: &str) -> Result<()> {
    if MARK_COLOURS.contains(&colour) {
        Ok(())
    } else {
        Err(Error::InvalidChange(format!(
            "\"{colour}\" is not a mark colour"
        )))
    }
}

fn bad_range(from: usize, to: usize) -> Error {
    Error::InvalidChange(format!("{from}..{to} is not a range in the text"))
}

/// The body text of message map `map` (created with every message; only
/// foreign data lacks it).
fn body_of(map: &LoroMap) -> Result<LoroText> {
    match map.get(BODY) {
        Some(ValueOrContainer::Container(Container::Text(body))) => Ok(body),
        _ => Ok(map.insert_container(BODY, LoroText::new())?),
    }
}

/// The message maps of a `messages` list, in order (other values skipped).
fn message_maps(list: &LoroList) -> Vec<LoroMap> {
    (0..list.len())
        .filter_map(|i| match list.get(i) {
            Some(ValueOrContainer::Container(Container::Map(map))) => Some(map),
            _ => None,
        })
        .collect()
}

fn value(map: &LoroMap, key: &str) -> Option<LoroValue> {
    match map.get(key) {
        Some(ValueOrContainer::Value(v)) => Some(v),
        _ => None,
    }
}

fn flag(map: &LoroMap, key: &str) -> bool {
    matches!(value(map, key), Some(LoroValue::Bool(true)))
}

fn int(map: &LoroMap, key: &str) -> Option<i64> {
    match value(map, key) {
        Some(LoroValue::I64(i)) => Some(i),
        _ => None,
    }
}

fn string(map: &LoroMap, key: &str) -> Option<String> {
    match value(map, key) {
        Some(LoroValue::String(s)) => Some(s.to_string()),
        _ => None,
    }
}

/// Now, in Unix seconds.
fn now() -> i64 {
    i64::try_from(unix_secs(SystemTime::now())).unwrap_or(i64::MAX)
}
