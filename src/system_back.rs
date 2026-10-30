//! The system's Back: Android's Back key and gesture, and a browser's.
//!
//! The app moves between its screens by itself, so the page had no history
//! to go back through, and Back left the app: a browser went to the page
//! before, and Android 7 to 11 closed the app, and a session under way with
//! it. wry's activity takes Back to the webview's history whenever there is
//! one to go back through, and leaves it to Android only when there is none.
//! So while the app is away from Home, on another screen or in a session,
//! the page keeps one entry of the app's own on its history. Back takes it
//! off, and the app does what the screen's own way back does: any other
//! screen goes Home, and a session or a run under way pauses, as it would
//! with the app put behind, and Back again ends it as Exit ends it, keeping
//! what it has. Home keeps no entry, so Back there leaves the app as it
//! always has. Going Home by the app's own buttons takes the entry off again, or
//! Back would then have to be pressed twice to leave.
//!
//! Android 16 never hands Back to an app built for it, as Dust is: the
//! system puts the app behind, which pauses a session. The desktop has no
//! Back, and an iPhone's webview no gesture for one.

use std::cell::RefCell;
use std::rc::Rc;

use dioxus::prelude::*;

use crate::session_runtime::{finish_run, hold_run};
use crate::state::{AppCtx, Screen};

/// What the page's history is asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Put the app's entry on.
    Push,
    /// Take it off, going back past it.
    Back,
}

/// The app's entry on the page's history, kept in step with the screen.
#[derive(Debug, Default)]
pub struct BackEntry {
    /// Whether the app's entry is the one the page is on.
    on: bool,
    /// The app's own Backs the page has still to tell of. The page tells of
    /// each as it tells of the learner's, and these are none of theirs.
    own_backs: usize,
}

impl BackEntry {
    /// What the history has to do now that the app is `away` from Home, or
    /// back there: away, the entry is on; at Home, it is not.
    pub fn follow(&mut self, away: bool) -> Option<Step> {
        if away == self.on {
            return None;
        }
        self.on = away;
        if away {
            Some(Step::Push)
        } else {
            self.own_backs += 1;
            Some(Step::Back)
        }
    }

    /// The page went back, or forward, to an entry that is the app's own,
    /// or not. Whether that was the learner's Back, for the screen up to
    /// answer.
    pub fn popped(&mut self, on_own: bool) -> bool {
        self.on = on_own;
        if self.own_backs > 0 {
            self.own_backs -= 1;
            return false;
        }
        !on_own
    }
}

/// Keep the page's history in step with the screen, and answer Back.
/// Called once, from the app's root.
pub fn use_system_back(ctx: AppCtx) {
    let entry = use_hook(|| Rc::new(RefCell::new(BackEntry::default())));
    let on_popped = use_callback({
        let entry = entry.clone();
        move |on_own: bool| {
            let back = entry.borrow_mut().popped(on_own);
            if back {
                answer_back(ctx);
            }
            // A session's results are away from Home too, and Back has to
            // find an entry there as well.
            follow(ctx, &entry);
        }
    });
    listen(on_popped);
    use_effect(move || {
        let _ = ctx.screen.read();
        follow(ctx, &entry);
    });
}

/// Back on the screen up: what that screen's own way back does. A session
/// or run is held the first time, as Android before 12 never did, and
/// later ones put the app behind: Back to step away for a moment ends
/// nothing.
fn answer_back(ctx: AppCtx) {
    let screen = *ctx.screen.peek();
    match screen {
        Screen::Home => {}
        Screen::Training => {
            if !hold_run(ctx) {
                finish_run(ctx);
            }
        }
        Screen::Settings | Screen::Stats | Screen::Listen | Screen::Results => {
            ctx.go_to(Screen::Home);
        }
    }
}

fn follow(ctx: AppCtx, entry: &RefCell<BackEntry>) {
    let away = *ctx.screen.peek() != Screen::Home;
    let step = entry.borrow_mut().follow(away);
    if let Some(step) = step {
        take(step);
    }
}

/// The state that marks the app's own entry.
const MARK: &str = "dust-away";

/// Where the page keeps its listener, to take it off again.
const SLOT: &str = "__dustBack";

/// The script that tells the app, each time the page goes back or forward
/// through its history, whether it is now on the app's own entry. A
/// listener left on before is taken off first.
fn listen_script() -> String {
    format!(
        r#"if (window.{SLOT}) window.{SLOT}();
const heard = (e) => dioxus.send(e.state === "{MARK}");
window.addEventListener("popstate", heard);
window.{SLOT} = () => window.removeEventListener("popstate", heard);"#
    )
}

fn step_script(step: Step) -> String {
    match step {
        Step::Push => format!(r#"history.pushState("{MARK}", "");"#),
        Step::Back => "history.back();".to_string(),
    }
}

#[cfg(not(test))]
fn listen(on_popped: Callback<bool>) {
    use_hook(move || {
        spawn(async move {
            let mut page = document::eval(&listen_script());
            while let Ok(on_own) = page.recv::<bool>().await {
                on_popped.call(on_own);
            }
        })
    });
}

#[cfg(not(test))]
fn take(step: Step) {
    spawn(async move {
        let _ = document::eval(&step_script(step)).await;
    });
}

/// The page a test runs: how many of the app's entries its history holds
/// above the first, and what listens for it going back.
#[cfg(test)]
#[derive(Default)]
struct Page {
    entries: usize,
    listener: Option<Callback<bool>>,
}

#[cfg(test)]
thread_local! {
    static PAGE: RefCell<Page> = RefCell::default();
}

/// The app opening is a page loaded afresh, with no history of its own.
#[cfg(test)]
fn listen(on_popped: Callback<bool>) {
    use_hook(move || {
        PAGE.set(Page {
            entries: 0,
            listener: Some(on_popped),
        });
    });
}

#[cfg(test)]
fn take(step: Step) {
    let (entries, listener) = PAGE.with_borrow_mut(|page| {
        match step {
            Step::Push => page.entries += 1,
            Step::Back => page.entries = page.entries.saturating_sub(1),
        }
        (page.entries, page.listener)
    });
    // A browser tells of its going back later, never as it is asked.
    if step == Step::Back
        && let Some(listener) = listener
    {
        spawn(async move { listener.call(entries > 0) });
    }
}

/// Press Back on the page a test runs. Whether the page took it: one that
/// did not was left to the system, which leaves the app.
#[cfg(test)]
pub fn press_back() -> bool {
    let popped = PAGE.with_borrow_mut(|page| {
        if page.entries == 0 {
            return None;
        }
        page.entries -= 1;
        Some((page.entries > 0, page.listener))
    });
    let Some((on_own, listener)) = popped else {
        return false;
    };
    if let Some(listener) = listener {
        listener.call(on_own);
    }
    true
}

/// How many of the app's entries the history of the page a test runs holds.
#[cfg(test)]
pub fn entries() -> usize {
    PAGE.with_borrow(|page| page.entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persist::Store;
    use crate::testing::{Ui, contest_settings, run, test_settings};

    /// Away from Home the entry goes on, once, however many screens are
    /// passed through; at Home it comes off.
    #[test]
    fn the_entry_is_on_exactly_while_the_app_is_away_from_home() {
        let mut entry = BackEntry::default();
        assert_eq!(entry.follow(false), None);
        assert_eq!(entry.follow(true), Some(Step::Push));
        assert_eq!(entry.follow(true), None);
        assert_eq!(entry.follow(false), Some(Step::Back));
        assert_eq!(entry.follow(false), None);
    }

    /// The page tells of the app's own Back as it tells of the learner's:
    /// that one is no Back of theirs, the next is.
    #[test]
    fn the_apps_own_back_is_not_taken_for_the_learners() {
        let mut entry = BackEntry::default();
        entry.follow(true);
        entry.follow(false);
        assert!(!entry.popped(false), "the app's own");
        entry.follow(true);
        assert!(entry.popped(false), "the learner's");
        assert_eq!(entry.follow(true), Some(Step::Push), "on again");
    }

    /// Forward, in a browser, onto the app's entry is no Back: the app
    /// stays where it is, and takes the entry off again at Home.
    #[test]
    fn forward_onto_the_apps_entry_is_no_back() {
        let mut entry = BackEntry::default();
        entry.follow(true);
        assert!(entry.popped(false));
        assert!(!entry.popped(true));
        assert_eq!(entry.follow(false), Some(Step::Back));
    }

    /// Back on any screen but Home goes Home, as the screen's own way back
    /// does. Back at Home is the system's, which leaves the app.
    #[test]
    fn back_goes_home_from_every_other_screen_and_leaves_from_home() {
        run(|| async {
            for (button, screen) in [
                ("nav-settings", "settings"),
                ("nav-stats", "stats"),
                ("btn-listen-to-letters", "listen"),
            ] {
                let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
                assert_eq!(entries(), 0, "Home keeps no entry");
                ui.click(button);
                assert_eq!(ui.screen(), screen);
                assert_eq!(entries(), 1, "{screen}");
                assert!(ui.with_app(|_| press_back()), "{screen} took Back");
                assert_eq!(ui.screen(), "home", "Back from {screen}");
                assert_eq!(entries(), 0, "{screen}");
                assert!(!ui.with_app(|_| press_back()), "Back at Home leaves");
            }
        });
    }

    /// Going Home by the app's own buttons takes its entry off, however
    /// many screens it passed through: one Back at Home leaves, and the
    /// page telling of the entry going changes nothing.
    #[test]
    fn going_home_by_the_apps_own_buttons_takes_the_entry_off() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
            ui.click("nav-settings");
            ui.click("nav-stats");
            assert_eq!(entries(), 1);
            ui.click("nav-practice");
            ui.advance(100).await;
            assert_eq!(ui.screen(), "home");
            assert_eq!(entries(), 0);
            ui.click("nav-stats");
            assert_eq!(entries(), 1, "away again");
            assert!(ui.with_app(|_| press_back()));
            assert_eq!(ui.screen(), "home");
        });
    }

    /// Back in a session pauses it, as putting the app behind does, and
    /// says so; Back again ends it as Exit does, keeping what was answered,
    /// and Back on its results goes Home: no session is lost to Back.
    #[test]
    fn back_in_a_session_pauses_it_and_again_ends_it_as_exit_does() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 3;
            let (mut ui, recorder) = Ui::app_with_settings(settings);
            ui.click("btn-start-training");
            assert!(ui.run_until(5_000, |ui| ui.has("Your turn")).await);
            let sent = recorder.texts().first().cloned().expect("a group was sent");
            ui.type_into("group-input-0", &sent);
            assert!(ui.run_until(5_000, |ui| ui.has("group done")).await);
            assert!(ui.with_app(|_| press_back()));
            ui.advance(100).await;
            assert_eq!(ui.screen(), "training", "held, not ended");
            assert!(ui.has("Back again ends it"), "{}", ui.html());
            assert_eq!(entries(), 1, "an entry for the next Back");
            assert!(ui.with_app(|_| press_back()));
            assert!(ui.run_until(2_000, |ui| ui.screen() == "results").await);
            assert_eq!(ui.store.load_sessions().len(), 1, "kept");
            assert_eq!(entries(), 1, "the results keep an entry for Back");
            assert!(ui.with_app(|_| press_back()));
            assert_eq!(ui.screen(), "home");
            assert_eq!(entries(), 0);
        });
    }

    /// The harness has no page to run them on, so the scripts are what a
    /// test can see: the page tells, each time it goes back or forward,
    /// whether it is on the app's own entry, which the app's push marks as
    /// its own; and the listener comes off again, one left before first.
    #[test]
    fn the_page_tells_whether_it_is_on_the_apps_own_entry() {
        let script = listen_script();
        assert!(
            script.contains(r#"dioxus.send(e.state === "dust-away")"#),
            "{script}"
        );
        assert!(
            script.contains(r#"window.addEventListener("popstate", heard)"#),
            "{script}"
        );
        assert!(
            script.contains(r#"window.removeEventListener("popstate", heard)"#),
            "{script}"
        );
        let off = script.find("window.__dustBack();").expect("taken off");
        let on = script.find("addEventListener").expect("put on");
        assert!(off < on, "{script}");
        assert_eq!(
            step_script(Step::Push),
            r#"history.pushState("dust-away", "");"#
        );
        assert_eq!(step_script(Step::Back), "history.back();");
    }

    /// Back in a contest run pauses it as its Pause does, and Back again
    /// ends it as Exit does.
    #[test]
    fn back_in_a_contest_run_pauses_it_and_again_ends_it_as_exit_does() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app_with_settings(contest_settings());
            ui.click("btn-start-training");
            ui.advance(1_000).await;
            assert!(ui.with_app(|ctx| ctx.contest.peek().is_some()));
            assert!(ui.with_app(|_| press_back()));
            ui.advance(100).await;
            assert_eq!(ui.screen(), "training", "held, not ended");
            assert!(ui.with_app(
                |ctx| ctx.contest.peek().as_ref().is_some_and(|run| matches!(
                    run.phase,
                    cw_core::ContestPhase::Paused {
                        why: cw_core::PauseWhy::You
                    }
                ))
            ));
            assert!(ui.with_app(|_| press_back()));
            assert!(ui.run_until(2_000, |ui| ui.screen() == "results").await);
            assert!(ui.with_app(|ctx| ctx.contest.peek().is_none()));
            assert!(ui.with_app(|_| press_back()));
            assert_eq!(ui.screen(), "home");
        });
    }
}
