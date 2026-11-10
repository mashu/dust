use std::collections::HashSet;

use cw_core::{
    Completion, CountersKey, CountersUpdate, SessionResult, TrainingSettings, apply_auto_level,
    complete_result,
};
use dioxus::prelude::*;

use super::{AppCtx, Finished, Notice, Screen};
use crate::persist::record::{Record, RecordId};
use crate::persist::{CutOff, SessionKey, SoFar};

/// Store what finishing a session did to its level's run, and move the
/// level if the run earned it, saying so. A change the level has moved
/// away from since is not made, and nothing is said of it
/// ([`apply_auto_level`]). What it said, if anything.
pub fn settle_level(ctx: AppCtx, done: &Completion) -> Option<String> {
    let AppCtx {
        mut settings,
        toasts,
        ..
    } = ctx;
    let store = ctx.app().store;
    match &done.counters {
        CountersUpdate::Keep(key, counters) => store.save_auto_counters(key, *counters),
        CountersUpdate::Clear(keys) => store.clear_auto_counters(keys),
    }
    let change = done.change.as_ref()?;
    // Onto the settings as they are now, not as the session started with
    // them, so the level is all it moves. Stored by the app's settings
    // effect.
    if !apply_auto_level(&mut settings.write(), change) {
        return None;
    }
    let message = crate::format::auto_level::change_message(change);
    toasts.show(message.clone());
    Some(message)
}

/// Keep what the sessions and runs the app stopped under, `cut`, had done
/// by then, as Exit would have kept each ([`AppCtx::tell_of_cut_off`]), and
/// the sessions their marks carried that the store had yet to take: in the
/// history, where each finished, and a session of groups counted towards
/// its level's run, the level moved if the run earned it. One a mark
/// carried was counted as it ended, and is not again.
///
/// Each is kept once, or not at all: one the history holds already, by
/// when and on what day it finished, as the app stopped under it as it was
/// stored, is not kept again, nor one deleted by hand under the record it
/// would be stored as. Two marks that carry one session store it once:
/// each is stored where it finished ([`RecordId::placed`]). One that a
/// record the store could not read for now may hold, as it finished no
/// later than such a record is named ([`AppCtx::unread_for_now_finished`]),
/// waits with this copy's mark until the record reads, when the history
/// holds it, or is no more, or is told of as one that could not be read,
/// when it is kept beside it ([`AppCtx::wait_for_unread`]): kept at once,
/// it would be held twice once the record read.
pub async fn keep_cut_off(ctx: AppCtx, cut: Vec<CutOff>) {
    let store = ctx.app().store;
    // Each as it is to be stored, on today's scale, as every session of the
    // history is read, with the settings a session of groups is judged by.
    let mut kept: Vec<(Record, SessionResult, Option<TrainingSettings>)> = Vec::new();
    let mut waiting = Vec::new();
    {
        let history = ctx.sessions.peek();
        let unread = ctx.unread_for_now_finished();
        let mut keys = HashSet::new();
        let so_far = cut.iter().filter_map(CutOff::so_far).map(|so_far| {
            let settings = so_far.settings.clone().map(TrainingSettings::clamp);
            (so_far.result.clone(), settings)
        });
        let carried = cut
            .iter()
            .flat_map(CutOff::unstored)
            .map(|session| (session.clone(), None));
        for (mut result, settings) in so_far.chain(carried) {
            let key = SessionKey::of(&result);
            if history.has_key(&key) || !keys.insert(key) {
                continue;
            }
            if unread.is_some_and(|through| result.timestamp <= through) {
                waiting.push(SoFar { result, settings });
                continue;
            }
            result.score = result.points();
            let Some(record) = Record::of(&result, |json| RecordId::placed(result.timestamp, json))
            else {
                continue;
            };
            kept.push((record, result, settings));
        }
    }
    ctx.wait_for_unread(waiting);
    if kept.is_empty() {
        ctx.let_go_of_carried(&cut);
        return;
    }
    // In the history, and held with this copy's mark as sessions the store
    // has yet to take, before the store is asked anything: the marks they
    // came from went from it once this copy's mark held what they carried,
    // which it has held since only for their sake
    // ([`AppCtx::let_go_of_carried`]).
    let put = kept
        .iter()
        .map(|(record, result, _)| (record.clone(), result.clone()))
        .collect();
    let held = ctx.hold_from_elsewhere(put);
    ctx.let_go_of_carried(&cut);
    if held.is_empty() {
        return;
    }
    // One deleted by hand under the record it would be stored as is let
    // go again, and so is one deleted here meanwhile.
    let ids = held.iter().map(|record| record.id).collect();
    let gone: HashSet<RecordId> = store.forgotten(ids).await.into_iter().collect();
    ctx.let_go_of_held(&gone);
    let held: Vec<Record> = {
        let history = ctx.sessions.peek();
        held.into_iter()
            .filter(|record| !gone.contains(&record.id) && history.index_of(record.id).is_some())
            .collect()
    };
    if held.is_empty() {
        return;
    }
    // The levels moved by them before they are stored: the app stopping
    // from here on keeps each once, its level counted once.
    for (record, result, settings) in kept {
        let Some(settings) = settings else {
            continue;
        };
        if held.iter().all(|held| held.id != record.id) {
            continue;
        }
        let run = store.load_auto_counters(&CountersKey::of(&settings));
        let done = complete_result(result, &settings, run);
        settle_level(ctx, &done);
    }
    let mut notices = ctx.notices;
    ctx.store_held(held, move |saved| {
        store.ask_to_keep();
        if let Some(problem) = saved.problem() {
            notices.write().push(Notice::History(problem));
        }
    });
}

/// Add `finished` to the history, store it, and show it on the results
/// screen, with the record it is found by there: a session of groups and a
/// contest run alike. Its mark stays, as it holds it, until the store has
/// taken it, so the app stopping before then, or a store with no room for
/// it, still keeps it.
pub fn keep_and_show(ctx: AppCtx, finished: SessionResult) {
    let AppCtx {
        mut screen,
        mut result,
        mut notices,
        ..
    } = ctx;
    ctx.mark_kept();
    let store = ctx.app().store;
    let kept = ctx.keep_finished(finished.clone(), move |saved| {
        // A browser may clear its store to make room; there is now
        // something in it worth asking it to keep.
        store.ask_to_keep();
        // Up until it is read: a session not stored is a session at risk.
        if let Some(problem) = saved.problem() {
            notices.write().push(Notice::History(problem));
        }
    });
    result.set(Some(Finished::Kept(Box::new(finished), kept)));
    screen.set(Screen::Results);
    // Again starts from a history one session longer.
    ctx.warm_sampler();
}
