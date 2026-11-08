//! A tiny cloud in the corner, there only while sync has something to
//! say: breathing while a sync is under way, a check for a moment once it
//! is done, a mark for a moment where it went wrong (the Sync card, on the
//! History tab of Stats, says what), and, where a browser's sign-in to
//! Google ran out, a cloud to tap to sign in again from wherever you are.
//! The app syncs by itself, a moment after a session is kept, so this is
//! how you see it did. It never shows over a session, and takes no tap but
//! the one that signs in, so it is never in the way.

use dioxus::prelude::*;

use crate::state::AppCtx;
use crate::state::SyncStatus;
use crate::time::sleep_ms;
use crate::ui::widgets::{Icon, IconName, control_id};

/// How long the check stays once a sync is done.
pub const SYNCED_MS: u32 = 1_600;
/// How long the mark stays where a sync went wrong: long enough to see,
/// as the card keeps what went wrong.
pub const TROUBLE_MS: u32 = 4_000;
/// How long the cloud takes to fade once its moment is over.
pub const FADE_MS: u32 = 400;

/// What the corner shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pip {
    Syncing,
    Synced,
    Trouble,
    /// Only a tap can sign in to Google again, as a browser opens its
    /// window only for one.
    SignIn,
}

impl Pip {
    /// What the corner shows on `status`, given whether a sync was under
    /// way when it last looked, and what it shows now: None to show
    /// nothing, and how long a moment lasts where it is one.
    pub fn after(
        status: &SyncStatus,
        was_syncing: bool,
        now: Option<Self>,
    ) -> (Option<Self>, Option<u32>) {
        match status {
            SyncStatus::On { syncing: true, .. } => (Some(Self::Syncing), None),
            SyncStatus::On {
                needs_sign_in: true,
                ..
            } => (Some(Self::SignIn), None),
            SyncStatus::On { problem, .. } if was_syncing => {
                if problem.is_some() {
                    (Some(Self::Trouble), Some(TROUBLE_MS))
                } else {
                    (Some(Self::Synced), Some(SYNCED_MS))
                }
            }
            // A moment still showing runs out by itself; anything else
            // the status no longer says goes.
            SyncStatus::On { .. } => match now {
                Some(Self::Synced | Self::Trouble) => (now, None),
                _ => (None, None),
            },
            _ => (None, None),
        }
    }

    fn class(self) -> &'static str {
        match self {
            Self::Syncing => "sync-pip syncing",
            Self::Synced => "sync-pip synced",
            Self::Trouble => "sync-pip trouble",
            Self::SignIn => "sync-pip sign-in",
        }
    }

    /// What it says, to a screen reader and on hover.
    fn says(self) -> &'static str {
        match self {
            Self::Syncing => "Syncing",
            Self::Synced => "Synced",
            Self::Trouble => {
                "Sync ran into a problem: the Sync card, on the History tab of Stats, says what"
            }
            Self::SignIn => "Sign in to Google to sync",
        }
    }
}

/// The corner, on the app's sync.
#[component]
pub fn SyncCorner() -> Element {
    let ctx = use_context::<AppCtx>();
    rsx! {
        SyncPip { status: ctx.sync.status, on_sign_in: move |()| ctx.sync_sign_in() }
    }
}

/// The corner on `status`, a tap on it signing in where only that can.
#[component]
pub fn SyncPip(status: ReadSignal<SyncStatus>, on_sign_in: EventHandler<()>) -> Element {
    let mut shown = use_signal(|| None::<Pip>);
    let mut leaving = use_signal(|| false);
    let mut was_syncing = use_signal(|| false);
    // Which moment is showing, so a timer of one before it takes nothing down.
    let mut moment = use_signal(|| 0_u64);
    use_effect(move || {
        let now = status.read().clone();
        let before = *shown.peek();
        let (next, lasts) = Pip::after(&now, *was_syncing.peek(), before);
        was_syncing.set(matches!(now, SyncStatus::On { syncing: true, .. }));
        // A moment still showing, left to run out.
        if lasts.is_none() && next == before && matches!(next, Some(Pip::Synced | Pip::Trouble)) {
            return;
        }
        // Anything else ends the moment showing, if one is: its timer
        // takes nothing down.
        let mine = *moment.peek() + 1;
        moment.set(mine);
        if *leaving.peek() {
            leaving.set(false);
        }
        if before != next {
            shown.set(next);
        }
        let Some(lasts) = lasts else {
            return;
        };
        spawn(async move {
            sleep_ms(lasts).await;
            if *moment.peek() != mine {
                return;
            }
            leaving.set(true);
            sleep_ms(FADE_MS).await;
            if *moment.peek() == mine {
                shown.set(None);
                leaving.set(false);
            }
        });
    });
    let Some(pip) = shown() else {
        return rsx! {};
    };
    let class = if leaving() {
        format!("{} leaving", pip.class())
    } else {
        pip.class().to_string()
    };
    let says = pip.says();
    if pip == Pip::SignIn {
        return rsx! {
            button {
                id: control_id("btn", "sync sign in"),
                class,
                r#type: "button",
                title: says,
                aria_label: says,
                onclick: move |_| on_sign_in.call(()),
                Icon { name: IconName::Cloud }
            }
        };
    }
    rsx! {
        div { id: "sync-pip", class, role: "status", title: says,
            Icon { name: IconName::Cloud }
            if pip == Pip::Synced {
                span { class: "sync-pip-mark", Icon { name: IconName::Check } }
            }
            if pip == Pip::Trouble {
                span { class: "sync-pip-mark", "!" }
            }
            span { class: "sr-only", "{says}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::testing::{Ui, run};

    thread_local! {
        static SIGNED_IN: Cell<usize> = const { Cell::new(0) };
    }

    /// Sync as the app goes through it, a moment after the page opens:
    /// each of `steps` after the one before, by how long and to what.
    #[component]
    fn Through(steps: Vec<(u32, SyncStatus)>) -> Element {
        let mut status = use_signal(|| on(false, None, false));
        use_future(move || {
            let steps = steps.clone();
            async move {
                for (after, next) in steps {
                    sleep_ms(after).await;
                    status.set(next);
                }
            }
        });
        rsx! {
            SyncPip { status, on_sign_in: |()| SIGNED_IN.with(|count| count.set(count.get() + 1)) }
        }
    }

    fn through(steps: Vec<(u32, SyncStatus)>) -> Ui {
        Ui::new(Through, ThroughProps { steps })
    }

    /// A sync shows in the corner as it runs, a check once it is done,
    /// and then fades and is gone; the corner was empty before.
    #[test]
    fn a_sync_shows_in_the_corner_and_goes() {
        run(|| async {
            let mut ui = through(vec![
                (100, on(true, None, false)),
                (500, on(false, None, false)),
            ]);
            assert!(!ui.shows("sync-pip"), "nothing to say yet");
            ui.advance(150).await;
            assert!(ui.has("sync-pip syncing"), "{}", ui.html());
            assert!(ui.has(">Syncing<"));
            ui.advance(500).await;
            assert!(ui.has("sync-pip synced"));
            assert!(ui.has("sync-pip-mark"));
            ui.advance(u64::from(SYNCED_MS)).await;
            assert!(ui.has("sync-pip synced leaving"));
            ui.advance(u64::from(FADE_MS) + 50).await;
            assert!(!ui.shows("sync-pip"));
        });
    }

    /// A sync that starts while the last one's check is up is shown, and
    /// the check's moment running out takes nothing down.
    #[test]
    fn a_sync_in_the_last_ones_moment_is_shown_through() {
        run(|| async {
            let mut ui = through(vec![
                (100, on(true, None, false)),
                (100, on(false, None, false)),
                (500, on(true, None, false)),
            ]);
            ui.advance(250).await;
            assert!(ui.has("sync-pip synced"));
            ui.advance(500).await;
            assert!(ui.has("sync-pip syncing"));
            ui.advance(u64::from(SYNCED_MS + FADE_MS) + 100).await;
            assert!(ui.has("sync-pip syncing"), "still under way: {}", ui.html());
        });
    }

    /// Gone wrong, a mark for a little longer; signed out, a cloud that
    /// stays, and signs in at a tap.
    #[test]
    fn trouble_is_marked_and_a_sign_in_is_a_tap_away() {
        run(|| async {
            let mut ui = through(vec![
                (100, on(true, None, false)),
                (100, on(false, Some("offline"), false)),
            ]);
            ui.advance(250).await;
            assert!(ui.has("sync-pip trouble"));
            ui.advance(u64::from(SYNCED_MS + FADE_MS)).await;
            assert!(ui.has("sync-pip trouble"), "longer than a check");
            ui.advance(u64::from(TROUBLE_MS)).await;
            assert!(!ui.shows("sync-pip"));

            SIGNED_IN.with(|count| count.set(0));
            let mut ui = through(vec![
                (100, on(true, None, false)),
                (100, on(false, None, true)),
            ]);
            ui.advance(250).await;
            assert!(ui.has("sync-pip sign-in"));
            ui.advance(10_000).await;
            assert!(ui.has("sync-pip sign-in"), "until signed in");
            ui.click("btn-sync-sign-in");
            assert_eq!(SIGNED_IN.with(Cell::get), 1);
        });
    }

    fn on(syncing: bool, problem: Option<&str>, needs_sign_in: bool) -> SyncStatus {
        SyncStatus::On {
            last: None,
            syncing,
            problem: problem.map(str::to_string),
            needs_sign_in,
        }
    }

    /// A sync under way breathes; once done, a check for a moment, or a
    /// mark where it went wrong; signed out, a cloud to tap that stays;
    /// and nothing at all where sync is off or nothing has happened.
    #[test]
    fn the_corner_shows_what_sync_is_doing() {
        assert_eq!(
            Pip::after(&on(true, None, false), false, None),
            (Some(Pip::Syncing), None)
        );
        assert_eq!(
            Pip::after(&on(false, None, false), true, Some(Pip::Syncing)),
            (Some(Pip::Synced), Some(SYNCED_MS))
        );
        assert_eq!(
            Pip::after(&on(false, Some("offline"), false), true, Some(Pip::Syncing)),
            (Some(Pip::Trouble), Some(TROUBLE_MS))
        );
        assert_eq!(
            Pip::after(&on(false, None, true), true, Some(Pip::Syncing)),
            (Some(Pip::SignIn), None)
        );
        assert_eq!(
            Pip::after(&on(false, None, false), false, None),
            (None, None)
        );
        assert_eq!(
            Pip::after(&on(false, None, false), false, Some(Pip::Synced)),
            (Some(Pip::Synced), None),
            "a moment runs out by itself"
        );
        assert_eq!(
            Pip::after(&on(false, None, false), false, Some(Pip::SignIn)),
            (None, None),
            "signed in again"
        );
        assert_eq!(
            Pip::after(&SyncStatus::Off { problem: None }, true, Some(Pip::Syncing)),
            (None, None)
        );
    }
}
