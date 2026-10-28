//! Who brought which changes into a document of a server-shared project, so
//! that "who wrote this" and the authors of review items rest on signatures,
//! not on the `peers` map alone (whose entries any member device can write).
//!
//! # Rule
//!
//! In a server-shared project, the op `(peer, counter)` of `main` or
//! `review` counts as written by device `D` of user `U` only if
//!
//! 1. the peer's ops up to and including it were all brought into this
//!    document by records or snapshots of `D`'s own stream of that document
//!    (signed by `D`, checked like every record), with no other blob that
//!    could have brought them; or `D` is this device and no import brought
//!    them at all (this device's own edits and files);
//! 2. the peer's `peers` entry names `D` and `U`;
//! 3. `D` is a device of `U` in the members log (`U` is taken from the log
//!    when the ops arrive).
//!
//! Anything else has an unknown author: never dropped, only not attributed.
//! An op another device forged under a peer of `D` (beyond what `D`'s stream
//! brought) is unknown, and so is every later op of that peer; a `peers`
//! entry naming another device than the one whose stream brought the peer's
//! ops makes them unknown. Local-only and folder-sync projects have no device
//! keys: there the `peers` map counts as it is (see the crate docs,
//! "Attribution").
//!
//! # Bookkeeping
//!
//! Per Loro peer, a [`PeerOrigin`]: the device (and its user) whose stream
//! brought the peer's first ops, how many of its ops (`end`, the counters
//! `0..end`) count as that device's, and whether more can still be credited
//! (`open`). Each import of a server record or snapshot compares the
//! document's version before and after it. For each peer that gained ops
//! `start..end`, the candidates are the blob itself (its range for that peer,
//! from `LoroDoc::decode_import_blob_meta`) and the ranges Loro parked
//! (pending) from earlier blobs, which an import can unlock. If exactly one
//! device's ranges cover the new ops and the peer's origin is that device,
//! still open and ending at `start`, the origin grows to `end` (a peer
//! without origin starts one at counter 0); otherwise the peer is closed and
//! keeps what it had. Before that, ops of the peer below `start` that no
//! import brought are taken as this device's own if the peer is (its entry
//! names this device, or it is the document's current peer) and its origin
//! is this device's or missing; otherwise the peer is closed too.
//!
//! A peer no import touched has no origin: it is this device's if its entry
//! names this device (local history, also of the time before the project was
//! shared), else unknown. Parked ranges live in memory only, like Loro's
//! pending changes. Origins are kept in the state file (see the `server`
//! module, "State").
//!
//! This works because every device's changes arrive in its own streams
//! only: pushes and compaction snapshots hold just the device's own ops
//! (see the `server` module, "Streams"). The one exception is the first
//! snapshot of the device that shared the project, which holds everything
//! the project held then: ops of other devices from before (folder sync,
//! another device in the same folder) stay unknown. Other known gaps, all
//! conservative (unknown, never someone else): projects shared before
//! origins were kept count only this device's own history; what was
//! imported after the last state write is unknown after a crash; an entry
//! another device overwrote (the `peers` map is last-writer-wins) makes its
//! peer unknown.

use std::collections::BTreeMap;

use loro::{Counter, IdSpan, LoroDoc, LoroMap, PeerID, VersionVector};
use serde::{Deserialize, Serialize};

use crate::{DeviceId, Identity, PEERS, UserId, peer_entry};

/// Where a Loro peer's ops came from (see the module docs), as kept in the
/// state file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerOrigin {
    /// The device whose stream brought them; empty: unknown.
    pub device: String,
    /// That device's user in the members log.
    pub user: String,
    /// The ops `0..end` count as the device's.
    pub end: Counter,
    /// Later ops can still be credited.
    pub open: bool,
}

impl PeerOrigin {
    fn unknown() -> Self {
        Self {
            device: String::new(),
            user: String::new(),
            end: 0,
            open: false,
        }
    }
}

/// Who wrote a peer's ops: `who` for counters below `until` (all of them
/// without).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PeerAuthor {
    pub(crate) who: Option<(UserId, DeviceId)>,
    pub(crate) until: Option<Counter>,
}

impl PeerAuthor {
    /// The author of the peer's op at `counter`.
    pub(crate) fn at(&self, counter: Counter) -> Option<&(UserId, DeviceId)> {
        self.who
            .as_ref()
            .filter(|_| self.until.is_none_or(|until| counter < until))
    }
}

/// The origins of one document's peers (see the module docs).
#[derive(Debug, Clone, Default)]
pub(crate) struct Trust {
    origins: BTreeMap<PeerID, PeerOrigin>,
    /// Ranges Loro parked from blobs: device, user, peer, counters.
    parked: Vec<(String, String, PeerID, Counter, Counter)>,
}

impl Trust {
    /// The origins kept in the state file (peer ids in decimal; others are
    /// ignored).
    pub(crate) fn from_settings(origins: Option<&BTreeMap<String, PeerOrigin>>) -> Self {
        let origins = origins
            .into_iter()
            .flatten()
            .filter_map(|(peer, origin)| Some((peer.parse().ok()?, origin.clone())))
            .collect();
        Self {
            origins,
            parked: Vec::new(),
        }
    }

    /// The origins for the state file.
    pub(crate) fn settings(&self) -> BTreeMap<String, PeerOrigin> {
        self.origins
            .iter()
            .map(|(peer, origin)| (peer.to_string(), origin.clone()))
            .collect()
    }

    /// Imports `update`, a record or snapshot of the stream of `device`
    /// (of user `user`), into `doc` and records where its new ops came from.
    pub(crate) fn import(
        &mut self,
        doc: &LoroDoc,
        update: &[u8],
        device: &str,
        user: &str,
        me: &Identity,
    ) -> loro::LoroResult<()> {
        // A blob Loro can't read imports nothing.
        let meta = LoroDoc::decode_import_blob_meta(update, false).ok();
        let before = doc.oplog_vv();
        let status = doc.import(update)?;
        let after = doc.oplog_vv();
        let peers = doc.get_map(PEERS);
        for (&peer, &end) in after.iter() {
            let start = before.get(&peer).copied().unwrap_or(0);
            if end <= start {
                continue;
            }
            self.settle(&peers, doc.peer_id(), me, peer, start);
            let mut ranges: Vec<(&str, &str, Counter, Counter)> = Vec::new();
            if let Some(m) = &meta
                && let Some(&to) = m.partial_end_vv.get(&peer)
            {
                let from = m.partial_start_vv.get(&peer).copied().unwrap_or(0);
                ranges.push((device, user, from, to));
            }
            for (d, u, p, from, to) in &self.parked {
                if *p == peer {
                    ranges.push((d, u, *from, *to));
                }
            }
            ranges.retain(|&(_, _, from, to)| from < end && to > start);
            let one = ranges
                .first()
                .map(|&(d, u, ..)| (d, u))
                .filter(|&(d, u)| ranges.iter().all(|r| (r.0, r.1) == (d, u)))
                .map(|(d, u)| (d.to_owned(), u.to_owned()));
            let source = one.filter(|_| covers(&mut ranges, start, end));
            match source {
                Some((d, u)) => self.credit(peer, start, end, d, u),
                None => self.close(peer),
            }
        }
        if let Some(pending) = &status.pending {
            for (&peer, &(from, to)) in pending.iter() {
                self.parked
                    .push((device.to_owned(), user.to_owned(), peer, from, to));
            }
        }
        self.parked
            .retain(|&(_, _, peer, _, to)| after.get(&peer).copied().unwrap_or(0) < to);
        Ok(())
    }

    /// Takes the ops of `peer` below `start` that no import brought as this
    /// device's, if the peer is its own; else closes the peer.
    fn settle(
        &mut self,
        peers: &LoroMap,
        current: PeerID,
        me: &Identity,
        peer: PeerID,
        start: Counter,
    ) {
        let origin = self.origins.get(&peer);
        if origin.map_or(0, |o| o.end) >= start {
            return;
        }
        let own = origin.is_none_or(|o| o.open && o.device == me.device.as_str())
            && is_mine(peers, current, me, peer);
        if own {
            let origin = self.origins.entry(peer).or_insert_with(|| PeerOrigin {
                device: me.device.as_str().to_owned(),
                user: me.user.as_str().to_owned(),
                end: 0,
                open: true,
            });
            origin.end = start;
        } else {
            self.close(peer);
        }
    }

    /// Credits the ops `start..end` of `peer` to `device` of `user`, or
    /// closes the peer if they don't continue its origin.
    fn credit(&mut self, peer: PeerID, start: Counter, end: Counter, device: String, user: String) {
        match self.origins.get_mut(&peer) {
            None if start == 0 => {
                self.origins.insert(
                    peer,
                    PeerOrigin {
                        device,
                        user,
                        end,
                        open: true,
                    },
                );
            }
            Some(o) if o.open && o.device == device && o.user == user && o.end == start => {
                o.end = end;
            }
            _ => self.close(peer),
        }
    }

    fn close(&mut self, peer: PeerID) {
        self.origins
            .entry(peer)
            .or_insert_with(PeerOrigin::unknown)
            .open = false;
    }

    /// Who wrote the ops of `peer` in `doc` (see the module docs, "Rule").
    pub(crate) fn author(&self, doc: &LoroDoc, me: &Identity, peer: PeerID) -> PeerAuthor {
        let entry = peer_entry(&doc.get_map(PEERS), peer);
        let mine = entry
            .as_ref()
            .is_some_and(|(u, d)| *u == me.user && *d == me.device);
        let Some(o) = self.origins.get(&peer) else {
            return PeerAuthor {
                who: entry.filter(|_| mine),
                until: None,
            };
        };
        let named = entry.as_ref().is_some_and(|(u, d)| {
            !o.device.is_empty() && u.as_str() == o.user && d.as_str() == o.device
        });
        let local = o.open && o.device == me.device.as_str() && mine;
        PeerAuthor {
            who: entry.filter(|_| named),
            until: (!local).then_some(o.end),
        }
    }

    /// This device's own peers in `doc` with how far their ops are its own:
    /// what it may push (ops another device forged under them are not).
    fn own_ends(&self, doc: &LoroDoc, me: &Identity) -> Vec<(PeerID, Counter)> {
        let peers = doc.get_map(PEERS);
        let current = doc.peer_id();
        let mut ends = Vec::new();
        for (&peer, &end) in doc.oplog_vv().iter() {
            let mine = is_mine(&peers, current, me, peer);
            let own = match self.origins.get(&peer) {
                None => mine.then_some(end),
                Some(o) if o.device == me.device.as_str() => {
                    Some(if o.open && mine { end } else { o.end.min(end) })
                }
                Some(_) => None,
            };
            ends.extend(own.map(|own| (peer, own)));
        }
        ends
    }

    /// The own peers' part of `doc`'s version (see [`Trust::own_ends`]).
    pub(crate) fn own_version(&self, doc: &LoroDoc, me: &Identity) -> VersionVector {
        let mut vv = VersionVector::new();
        for (peer, end) in self.own_ends(doc, me) {
            vv.set_end(loro::ID::new(peer, end));
        }
        vv
    }

    /// The spans of the own peers' ops in `doc` beyond `pushed`, and the
    /// version they bring the own peers to.
    pub(crate) fn unpushed(
        &self,
        doc: &LoroDoc,
        me: &Identity,
        pushed: &VersionVector,
    ) -> (Vec<IdSpan>, VersionVector) {
        let mut spans = Vec::new();
        let mut target = VersionVector::new();
        for (peer, end) in self.own_ends(doc, me) {
            let from = pushed.get(&peer).copied().unwrap_or(0);
            if end > from {
                spans.push(IdSpan::new(peer, from, end));
            }
            target.set_end(loro::ID::new(peer, end.max(from)));
        }
        (spans, target)
    }

    /// Whether `peer` is contested: an import brought ops under it that
    /// aren't this device's (see [`crate::Document`]'s peer renewal).
    pub(crate) fn contested(&self, me: &Identity, peer: PeerID) -> bool {
        self.origins
            .get(&peer)
            .is_some_and(|o| !o.open || o.device != me.device.as_str())
    }
}

/// Whether `peer` is this device's own: its `peers` entry names this device
/// and user, or it is the document's current peer `current`.
fn is_mine(peers: &LoroMap, current: PeerID, me: &Identity, peer: PeerID) -> bool {
    peer == current || peer_entry(peers, peer).is_some_and(|(u, d)| u == me.user && d == me.device)
}

/// Whether `ranges` (sorted here) together cover `start..end`.
fn covers(ranges: &mut [(&str, &str, Counter, Counter)], start: Counter, end: Counter) -> bool {
    ranges.sort_by_key(|r| r.2);
    let mut at = start;
    for &(_, _, from, to) in ranges.iter() {
        if from > at {
            return false;
        }
        at = at.max(to);
        if at >= end {
            return true;
        }
    }
    at >= end
}
