//! What is told of records this build cannot read, and what is noted of
//! them so that each is told of once: the rules, over what the notes hold,
//! whatever keeps them. Each note names records ([`RecordId::name`]):
//!
//! - [`Note::Told`]: every record told of, of either kind, beside what a
//!   build before formats noted there, the records it could not open or
//!   make out. This build only adds names to it: dropped, such a build
//!   would tell of one again as it next opened.
//! - [`Note::Failing`]: the records that could not be read at all as the
//!   history was last read whole, or at the app's last try at one, and have
//!   not read since, nor gone: one that still cannot be read as it is read
//!   again, at a later launch, is told of then.
//! - [`Note::UnreadTold`]: the records told of as ones that could not be
//!   read at all, until each reads, or is gone.
//! - [`Note::DamagedTold`]: the records told of as damaged, or as a newer
//!   Dust's, until each reads, or is gone. Until it is first written,
//!   [`Note::Told`] stands for it; written, it stays, though it names none.
//!
//! A store keeps what it can of them: the files keep all four; a browser's
//! database the first alone, as it notes no failure, and reads no record a
//! moment's damage leaves; the test store the other three.

use std::collections::{BTreeSet, HashSet};

use super::super::record::{
    HistoryNotice, Record, RecordId, Unopened, Unreadable, names_noted, not_told,
};

/// A note of records this build could not read, kept beside the history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Note {
    Told,
    Failing,
    UnreadTold,
    DamagedTold,
}

/// The notes a look that lists the history keeps up, in the order it keeps
/// them: each loses the records the listing no longer holds
/// ([`gone_from`]).
pub(crate) const LISTED: [Note; 3] = [Note::Failing, Note::UnreadTold, Note::DamagedTold];

/// What each note holds, as it was read: none, where it is not there, or
/// could not be read.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Held<'a> {
    pub(crate) told: Option<&'a [u8]>,
    pub(crate) failing: Option<&'a [u8]>,
    pub(crate) unread_told: Option<&'a [u8]>,
    pub(crate) damaged_told: Option<&'a [u8]>,
}

/// What a read of the whole history tells, and what it then notes
/// ([`at_open`]).
#[derive(Debug, PartialEq)]
pub(crate) struct Told {
    pub(crate) notices: Vec<HistoryNotice>,
    /// Each note to write, in order, and what it is to hold: none, where it
    /// is to go.
    pub(crate) noted: Vec<(Note, Option<BTreeSet<String>>)>,
}

/// What to tell of `unread`, the records this build cannot read, each with
/// what it says of itself, and of `failed`, those that could not be read at
/// all just now, `held` being the notes as they were read: of the second,
/// only those that could not be read as the history was last read either,
/// and have not read since ([`Note::Failing`]), as a moment's failure is
/// over by the app's first try at reading it again; and nothing, once each
/// has been told of. And what is to be noted then, where the store may
/// write, so that each is told of once, and what failed, for the next read
/// to tell of.
///
/// Each kind of notice goes by a note of its own. One of a record this
/// build cannot read, by [`Note::DamagedTold`], which keeps each of those
/// it cannot read now and each it told of that fails now, and drops the
/// rest, as read or gone; until it is first written, [`Note::Told`] stands
/// for it, and from then on a record a build before formats told of first
/// is told of again here. One of a failure, by [`Note::UnreadTold`], which
/// keeps each told of before, by this read or by the app's tries
/// ([`still_unread`]), that still fails, and drops one that reads now or is
/// gone: should it fail again, that is a failure of its own, told of in
/// turn. [`Note::Told`] only gains names: every one told of, of either
/// kind, beside what a build before formats noted there. Such a build
/// writes it over, as it tells of a record it does not name, with every
/// record it cannot read just then.
pub(crate) fn at_open(
    unread: &[(RecordId, Unreadable)],
    failed: &[RecordId],
    held: Held<'_>,
) -> Told {
    let failed_before = names_noted(held.failing);
    let unread_now: Vec<(RecordId, Option<Unreadable>)> = unread
        .iter()
        .map(|(id, said)| (*id, Some(said.clone())))
        .collect();
    let failed_now: Vec<(RecordId, Option<Unreadable>)> = failed
        .iter()
        .filter(|id| failed_before.contains(&id.name()))
        .map(|id| (*id, None))
        .collect();
    // Of a record this build cannot read, and of a failure, as this build
    // told of it, whatever a build before this one noted of the record; of
    // the first, as such a build noted it, until this build first notes one.
    let mut notices = not_told(&unread_now, held.damaged_told.or(held.told));
    notices.extend(not_told(&failed_now, held.unread_told));
    let mut noted = Vec::new();
    // What it noted, with what it tells of now, whether or not there is
    // anything new to tell: what a build before this one noted there stays.
    // A failure told of, now or before, is noted there too, where such a
    // build looks, so that it does not tell of it again.
    let told_before = names_noted(held.told);
    let names: BTreeSet<String> = told_before
        .iter()
        .cloned()
        .chain(unread_now.iter().map(|(id, _)| id.name()))
        .chain(failed_now.iter().map(|(id, _)| id.name()))
        .collect();
    let told_more = names.len() != told_before.len();
    if told_more {
        noted.push((Note::Told, Some(names)));
    }
    // Of the failures told of, before or now, those that still fail.
    let unread_told_before = names_noted(held.unread_told);
    let failing_now: HashSet<String> = failed.iter().map(|id| id.name()).collect();
    let still_told: BTreeSet<String> = unread_told_before
        .iter()
        .filter(|name| failing_now.contains(*name))
        .cloned()
        .chain(failed_now.iter().map(|(id, _)| id.name()))
        .collect();
    if still_told.len() != unread_told_before.len()
        || still_told
            .iter()
            .any(|name| !unread_told_before.contains(name))
    {
        noted.push((
            Note::UnreadTold,
            (!still_told.is_empty()).then_some(still_told),
        ));
    }
    // Of the records this build cannot read, every one it cannot read now,
    // told of now or before, and every one it told of that fails now: one
    // that reads, or is gone, is told of no more. Written the first time
    // where the note of what was told is there, or is written now, though
    // it names none, so that that note stands for it no more: what this
    // build notes there is no build before's.
    let damaged_before = names_noted(held.damaged_told);
    let damaged_now: BTreeSet<String> = unread_now
        .iter()
        .map(|(id, _)| id.name())
        .chain(
            damaged_before
                .iter()
                .filter(|name| failing_now.contains(*name))
                .cloned(),
        )
        .collect();
    let changed = damaged_now.len() != damaged_before.len()
        || damaged_now
            .iter()
            .any(|name| !damaged_before.contains(name));
    let first = held.damaged_told.is_none() && (held.told.is_some() || told_more);
    if first || changed {
        noted.push((Note::DamagedTold, Some(damaged_now)));
    }
    if failing_now != failed_before {
        let failing: BTreeSet<String> = failing_now.into_iter().collect();
        noted.push((Note::Failing, (!failing.is_empty()).then_some(failing)));
    }
    Told { notices, noted }
}

/// What a read of the records `read` takes out of the notes, note by note,
/// in the order they are kept up: those read, from the note of damage, as
/// damaged no more; those read, or found damaged, from the notes of
/// failures, as failing no more, so that one of them that fails for a
/// moment again as the history is next read whole is not told of then, and
/// one told of that fails again after, a failure of its own, is told of in
/// turn ([`no_more`]).
pub(crate) fn after_read(read: &[Result<Record, Unopened>]) -> [(Note, HashSet<String>); 3] {
    let read_now: HashSet<String> = read
        .iter()
        .filter_map(|read| read.as_ref().ok().map(|record| record.id.name()))
        .collect();
    let damaged = read.iter().filter_map(|read| match read {
        Err(Unopened::Damaged(id)) => Some(id.name()),
        _ => None,
    });
    let failing_no_longer: HashSet<String> = read_now.iter().cloned().chain(damaged).collect();
    [
        (Note::DamagedTold, read_now),
        (Note::Failing, failing_no_longer.clone()),
        (Note::UnreadTold, failing_no_longer),
    ]
}

/// Of what a note, `raw`, names, those a look that listed the history,
/// `listed` being every record it found, no longer holds: they are gone,
/// and should one come back and fail, that is a failure of its own, told
/// of in turn ([`LISTED`], [`no_more`]).
pub(crate) fn gone_from(raw: &[u8], listed: &HashSet<String>) -> HashSet<String> {
    names_noted(Some(raw))
        .into_iter()
        .filter(|name| !listed.contains(name))
        .collect()
}

/// What `note`, holding `raw`, is to hold once `gone`, records by name, are
/// taken out of it: nothing, where it names none of them, and it is left as
/// it is; none, where it is to go, as one left naming none goes, but for
/// [`Note::DamagedTold`], which stays, empty: from its first writing it
/// alone holds back a notice of damage. Only this build's own notes lose
/// names: what a build before this one noted in [`Note::Told`], the records
/// it cannot read, is no failure this build noted, and stays as it is, as
/// that build would tell of it again as it next opens.
pub(crate) fn no_more(
    note: Note,
    raw: &[u8],
    gone: &HashSet<String>,
) -> Option<Option<BTreeSet<String>>> {
    let noted = names_noted(Some(raw));
    let left: BTreeSet<String> = noted
        .iter()
        .filter(|name| !gone.contains(*name))
        .cloned()
        .collect();
    if left.len() == noted.len() {
        return None;
    }
    Some((!left.is_empty() || note == Note::DamagedTold).then_some(left))
}

/// What to tell of `ids`, records listed that could not be read at all at
/// any of the app's tries since it found them so, `unread_told` being the
/// note of those this build told of as such ([`Note::UnreadTold`]): of
/// those not named there, how many ([`HistoryNotice::Unread`]); and, where
/// there are any, that note with every one of `ids` added, to be written
/// where the store may write, so each is told of once. Each is noted in
/// [`Note::Told`] too, so that a build before formats does not tell of it
/// again, and in [`Note::Failing`], as this build's failures
/// ([`noted_unread`]). What a build before this one noted of one in
/// [`Note::Told`] holds back nothing: such a build notes there every record
/// it could not open, a moment's failure among them, and one that has read
/// since fails now on its own.
pub(crate) fn still_unread(
    ids: &[RecordId],
    unread_told: Option<&[u8]>,
) -> (Vec<HistoryNotice>, Option<BTreeSet<String>>) {
    let still: Vec<(RecordId, Option<Unreadable>)> = ids.iter().map(|id| (*id, None)).collect();
    let notices = not_told(&still, unread_told);
    if notices.is_empty() {
        return (notices, None);
    }
    let mut names: BTreeSet<String> = names_noted(unread_told).into_iter().collect();
    names.extend(ids.iter().map(|id| id.name()));
    (notices, Some(names))
}

/// A note, `raw`, with `ids` added, where it gains a name by it: none,
/// where it names them all already, and is left as it is
/// ([`still_unread`]).
pub(crate) fn noted_unread(ids: &[RecordId], raw: Option<&[u8]>) -> Option<BTreeSet<String>> {
    let mut names: BTreeSet<String> = names_noted(raw).into_iter().collect();
    let held = names.len();
    names.extend(ids.iter().map(|id| id.name()));
    (names.len() != held).then_some(names)
}
