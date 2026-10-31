//! The history as the screens that show it share it: one handle, cheap to
//! clone and to compare, made anew with each change to the history and with
//! no other, and kept with it every figure worked out of it so far.
//!
//! Stats and Home are mounted afresh each time they are opened, and each of
//! their figures reads the whole history: replaying it, as the copy span
//! does, or reading every run's log, as the score board does. So a figure
//! is worked out the first time a screen asks for it of a history, for what
//! of the settings it reads, and kept with the handle: a screen opened again
//! on the same history, or a tab shown again, finds it worked out. The
//! history is read where the app keeps it, never copied.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use cw_core::{BandSettings, SessionResult, TrainingSettings};
use dioxus::prelude::*;

use crate::state::{AppCtx, in_root};
use crate::time::{as_shown, now_ms};

/// How many of one figure a history keeps, each for other settings or
/// another choice on the screen: the newest.
const KEPT_OF_A_FIGURE: usize = 8;

/// A history, and what has been worked out of it. Two handles are equal
/// only when they are the same handle: what they hold is never compared.
#[derive(Clone)]
pub struct History(Rc<Held>);

struct Held {
    sessions: Sessions,
    figures: RefCell<Vec<Figure>>,
}

enum Sessions {
    /// The app's history, read where the app keeps it.
    App(Signal<Vec<SessionResult>>),
    /// A history of its own: the app's as the clock reads it, while a
    /// session is stamped ahead of it, or one a screen is drawn with alone.
    Own(Vec<SessionResult>),
}

/// One figure worked out of a history: which, for what, and what it came
/// to.
struct Figure {
    name: &'static str,
    key: Box<dyn Any>,
    value: Rc<dyn Any>,
}

impl History {
    fn new(sessions: Sessions) -> Self {
        Self(Rc::new(Held {
            sessions,
            figures: RefCell::new(Vec::new()),
        }))
    }

    /// The sessions, as they are stored, to read.
    pub fn with<T>(&self, read: impl FnOnce(&[SessionResult]) -> T) -> T {
        match &self.0.sessions {
            Sessions::App(sessions) => read(&sessions.peek()),
            Sessions::Own(sessions) => read(sessions),
        }
    }

    pub fn len(&self) -> usize {
        self.with(<[SessionResult]>::len)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The figure `name` of this history, for `key`: what `work_out` makes
    /// of the sessions the first time it is asked for, and the same figure
    /// after that, for as long as the history is the same. `key` is what of
    /// the settings, or of the screen, the figure reads, and nothing more:
    /// a figure that does not read the volume is not worked out again when
    /// the volume moves.
    pub fn figure<K, T>(
        &self,
        name: &'static str,
        key: K,
        work_out: impl FnOnce(&[SessionResult]) -> T,
    ) -> Rc<T>
    where
        K: PartialEq + 'static,
        T: 'static,
    {
        let found = self
            .0
            .figures
            .borrow()
            .iter()
            .find(|figure| {
                figure.name == name
                    && figure.key.downcast_ref::<K>() == Some(&key)
                    && figure.value.is::<T>()
            })
            .map(|figure| figure.value.clone());
        if let Some(value) = found.and_then(|value| value.downcast::<T>().ok()) {
            return value;
        }
        #[cfg(test)]
        crate::testing::worked_out(name);
        // The figures are not held while it is worked out: it may ask for
        // another.
        let value = Rc::new(self.with(work_out));
        let mut figures = self.0.figures.borrow_mut();
        let kept = figures.iter().filter(|figure| figure.name == name).count();
        if kept >= KEPT_OF_A_FIGURE
            && let Some(oldest) = figures.iter().position(|figure| figure.name == name)
        {
            figures.remove(oldest);
        }
        figures.push(Figure {
            name,
            key: Box::new(key),
            value: value.clone(),
        });
        value
    }

    /// The history as the screens show it ([`as_shown`]): this one, unless
    /// a session is stamped ahead of the clock; then one of its own, settled,
    /// worked out once for as long as the same sessions are ahead.
    pub fn shown(&self) -> History {
        let now = now_ms();
        let newest = *self.figure("newest stamp", (), |sessions| {
            sessions.iter().map(|session| session.timestamp).max()
        });
        if newest.is_none_or(|newest| newest <= now) {
            return self.clone();
        }
        // Those ahead are always the newest stamped, so how many there are
        // says which.
        let ahead = self.with(|sessions| {
            sessions
                .iter()
                .filter(|session| session.timestamp > now)
                .count()
        });
        History::clone(&self.figure("as shown", ahead, |sessions| {
            History::from(as_shown(sessions).into_owned())
        }))
    }
}

impl PartialEq for History {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

/// A history of its own, for a screen drawn with it alone.
impl From<Vec<SessionResult>> for History {
    fn from(sessions: Vec<SessionResult>) -> Self {
        Self::new(Sessions::Own(sessions))
    }
}

/// What a figure that reads the settings at large reads of `settings`: all
/// but the band's, which no figure of the history reads, so that the volume
/// or the noise moved works none of them out again.
pub fn without_band(settings: &TrainingSettings) -> TrainingSettings {
    TrainingSettings {
        band: BandSettings::default(),
        ..settings.clone()
    }
}

/// The app's history as the screens share it: a handle made anew with each
/// change to the history, and with no other change. It is made in the root
/// scope the first time a screen asks for it, so it lasts as long as the
/// app does, and a screen opened again finds it, and what was worked out of
/// it, as it was left.
pub fn use_history(ctx: AppCtx) -> Memo<History> {
    use_hook(|| {
        in_root(|| {
            try_consume_context::<Memo<History>>().unwrap_or_else(|| {
                provide_context(Memo::new(move || {
                    // Read, so that every change to the history makes a
                    // new handle; the handle reads it where it is.
                    let _ = ctx.sessions.read();
                    History::new(Sessions::App(ctx.sessions))
                }))
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Ui, run, stored_session, test_settings};
    use cw_core::CharSetMode;

    fn history() -> History {
        History::from(vec![
            stored_session("2026-09-01", &[("KM", "KM")]),
            stored_session("2026-09-02", &[("KM", "KX")]),
        ])
    }

    /// A figure is worked out once for each key, and found after that.
    #[test]
    fn a_figure_is_worked_out_once_for_each_key() {
        let history = history();
        let mut runs = 0;
        let mut count = |key: u8| {
            history.figure("count", key, |sessions| {
                runs += 1;
                sessions.len()
            })
        };
        let first = count(1);
        assert!(Rc::ptr_eq(&first, &count(1)));
        assert_eq!(*count(2), 2);
        assert_eq!(runs, 2);
        // The same name and key for a figure of another type is another
        // figure.
        let words = history.figure("count", 1_u8, |_| "two");
        assert_eq!(*words, "two");
    }

    /// Only the newest few of a figure are kept, each for its own key.
    #[test]
    fn a_figure_keeps_its_newest_keys() {
        let history = history();
        for key in 0..=KEPT_OF_A_FIGURE {
            history.figure("key", key, |_| key);
        }
        let mut again = false;
        history.figure("key", KEPT_OF_A_FIGURE, |_| {
            again = true;
            0_usize
        });
        assert!(!again, "the newest was kept");
        history.figure("key", 0, |_| {
            again = true;
            0_usize
        });
        assert!(again, "the oldest was let go");
    }

    /// Two handles are equal only when they are one: a copy of the same
    /// sessions is another history, with figures of its own.
    #[test]
    fn a_history_is_equal_only_to_itself() {
        let history = history();
        assert!(history == history.clone());
        assert!(history != self::history());
    }

    /// Nothing stamped ahead of the clock: the history is shown as it is.
    #[test]
    fn a_history_on_time_is_shown_as_it_is() {
        let history = history();
        assert!(history.shown() == history);
    }

    /// A session stamped ahead is shown settled, in a history of its own
    /// kept while the same sessions are ahead.
    #[test]
    fn a_session_stamped_ahead_is_shown_settled() {
        let mut ahead = stored_session("2099-01-01", &[("KM", "KM")]);
        ahead.timestamp = u64::MAX / 2;
        let mut sessions = vec![
            stored_session("2026-09-01", &[("KM", "KM")]),
            stored_session("2026-09-02", &[("KM", "KM")]),
        ];
        sessions.push(ahead);
        let history = History::from(sessions);
        let shown = history.shown();
        assert!(shown != history);
        assert!(shown == history.shown(), "kept");
        shown.with(|sessions| assert_eq!(sessions[2].date, "2026-09-02"));
        history.with(|sessions| assert_eq!(sessions[2].date, "2099-01-01"));
    }

    /// The Stats tabs, in the order the control offers them.
    const TABS: [&str; 6] = [
        "seg-overview",
        "seg-scores",
        "seg-letters",
        "seg-mistakes",
        "seg-sampling",
        "seg-history",
    ];

    /// `count` sessions of ten groups, a day apart, the oldest first: on
    /// the board, and long enough for every card.
    fn sessions(count: u64) -> Vec<SessionResult> {
        (1..=count)
            .map(|day| {
                let mut session = stored_session("2026-10-01", &[("KM", "KM"); 10]);
                session.timestamp = day * 86_400_000;
                session.date = format!("2026-{:02}-{:02}", 1 + day / 28, 1 + day % 28);
                session
            })
            .collect()
    }

    /// The app over `count` sessions, on Stats, every tab shown once and
    /// whatever was worked out for it, and for Home before it, counted.
    fn stats_seen(count: u64) -> Ui {
        let (mut ui, _recorder) = Ui::app_with_history(test_settings(), &sessions(count));
        ui.click("nav-stats");
        for tab in TABS {
            ui.click(tab);
        }
        ui
    }

    /// Each figure is worked out once, the first time a tab shows it; the
    /// tabs shown again, and Stats left and opened again, work nothing out
    /// again while the history is the same.
    #[test]
    fn switching_stats_tabs_recomputes_nothing_for_an_unchanged_history() {
        run(|| async {
            let mut ui = stats_seen(12);
            let first = ui.take_all_worked_out();
            for figure in [
                "overview",
                "span history",
                "score board",
                "letters",
                "mistakes",
            ] {
                assert!(first.contains(&(figure, 1)), "{figure}: {first:?}");
            }
            assert!(first.iter().all(|(_, count)| *count == 1), "{first:?}");

            for tab in TABS {
                ui.click(tab);
            }
            ui.click("nav-practice");
            ui.click("nav-stats");
            for tab in TABS {
                ui.click(tab);
            }
            assert_eq!(ui.take_all_worked_out(), []);
            assert!(ui.has("Session history"));
        });
    }

    /// The volume is read by no figure: moved, it works none out again.
    #[test]
    fn a_setting_no_figure_reads_works_none_out_again() {
        run(|| async {
            let mut ui = stats_seen(12);
            ui.take_all_worked_out();
            ui.with_app(|ctx| {
                let mut settings = ctx.settings;
                settings.write().band.master_volume = 0.5;
            });
            for tab in TABS {
                ui.click(tab);
            }
            assert_eq!(ui.take_all_worked_out(), []);
        });
    }

    /// A session kept is a new history: what a tab shows is worked out
    /// again, once.
    #[test]
    fn a_changed_history_works_its_figures_out_once_more() {
        run(|| async {
            let mut ui = stats_seen(12);
            ui.take_all_worked_out();
            ui.with_app(|ctx| {
                let mut sessions = ctx.sessions;
                sessions.write().extend(self::sessions(13).pop());
            });
            ui.click("seg-overview");
            assert_eq!(ui.take_worked_out("overview"), 1);
            assert!(ui.has("13 sessions"), "{}", ui.html());
            ui.click("nav-practice");
            ui.click("nav-stats");
            assert_eq!(ui.take_worked_out("overview"), 0);
        });
    }

    /// A mode pill works out the history of the mode it shows, for its card
    /// alone, once.
    #[test]
    fn a_mode_pill_works_out_only_its_cards_mode() {
        run(|| async {
            let mut history = sessions(12);
            for session in history.iter_mut().skip(6) {
                session.char_set_mode = CharSetMode::Mixed;
            }
            let (mut ui, _recorder) = Ui::app_with_history(test_settings(), &history);
            ui.click("nav-stats");
            ui.take_all_worked_out();
            ui.click("span-mixed");
            assert_eq!(ui.take_all_worked_out(), [("span history", 1)]);
            ui.click("span-koch");
            ui.click("span-mixed");
            assert_eq!(ui.take_all_worked_out(), []);
        });
    }

    /// Home works out its streak and calendar once for a history: opened
    /// again, it reads them; a session kept, it works them out again.
    #[test]
    fn home_works_out_its_figures_once_for_a_history() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app_with_history(test_settings(), &sessions(5));
            assert_eq!(ui.take_worked_out("practice days"), 1);
            ui.click("nav-stats");
            ui.click("nav-practice");
            ui.click("nav-settings");
            ui.click("nav-practice");
            assert_eq!(ui.take_worked_out("practice days"), 0);
            ui.with_app(|ctx| {
                let mut sessions = ctx.sessions;
                sessions.write().extend(self::sessions(6).pop());
            });
            assert_eq!(ui.take_worked_out("practice days"), 1);
        });
    }

    /// Stats is handed the app's own history, where the app keeps it: a
    /// handle the size of a pointer, which a redraw compares as one. The
    /// route drawn again for something else draws no tab again.
    #[test]
    fn stats_props_hold_no_copy_of_the_history() {
        run(|| async {
            assert_eq!(size_of::<History>(), size_of::<usize>());
            let (mut ui, _recorder) = Ui::app_with_history(test_settings(), &sessions(5));
            ui.click("nav-stats");
            let shared = ui.with_app(|ctx| {
                let history = in_root(consume_context::<Memo<History>>);
                let history = history.peek().clone();
                history.with(<[SessionResult]>::as_ptr) == ctx.sessions.peek().as_ptr()
            });
            assert!(shared, "Stats reads the sessions where the app keeps them");
            ui.take_renders("OverviewTab");
            ui.with_app(|ctx| {
                let mut last_backup = ctx.last_backup;
                last_backup.set(Some(1));
            });
            assert!(ui.has("Accuracy over time"));
            assert_eq!(ui.take_renders("OverviewTab"), 0);
        });
    }
}
