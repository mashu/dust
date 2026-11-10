//! How far the history read as the app opens has come ([`LoadProgress`]):
//! counted by the store as it reads the history, or moves it from where an
//! older build kept it whole, on whatever thread does that, and asked by
//! the boot screen, which says it once the read has taken a while
//! ([`HistoryProgress::said`]).

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

/// How long the boot screen shows the paper alone before it says how far
/// the history has come: most reads are over by then, and a screen that
/// flashed up for them would only be in the way. A browser's own boot
/// screen shows from the page's first frame, and gains the words then.
pub const BOOT_NOTE_AFTER_MS: u64 = 1_000;

/// Where a browser's boot screen says it, in `index.html`.
#[cfg(any(feature = "web", test))]
pub const BOOT_NOTE_ID: &str = "boot-note";

/// Nothing counted yet: how the history is kept is being read, say.
const OPENING: u8 = 0;
const READING: u8 = 1;
const MOVING: u8 = 2;
/// How many there are to read is not known.
const UNCOUNTED: u64 = u64::MAX;

/// How far the read of the history, or the move before it, has come: each
/// count made where the work is done, and read only to be shown, with
/// nothing else to keep in order with it.
#[derive(Debug, Default)]
pub struct LoadProgress {
    stage: AtomicU8,
    done: AtomicU64,
    of: AtomicU64,
}

impl LoadProgress {
    /// The read of `of` records begins: none, where how many there are is
    /// not known until they are read, as a browser's database may not say.
    pub fn reading(&self, of: Option<u64>) {
        self.begin(READING, of);
    }

    /// The move of `of` sessions begins: none, where it is made whole at
    /// once, and cannot be counted as it goes, as a browser makes it.
    pub fn moving(&self, of: Option<u64>) {
        self.begin(MOVING, of);
    }

    fn begin(&self, stage: u8, of: Option<u64>) {
        self.done.store(0, Ordering::Relaxed);
        self.of.store(of.unwrap_or(UNCOUNTED), Ordering::Relaxed);
        self.stage.store(stage, Ordering::Release);
    }

    /// One more read, or moved.
    #[cfg_attr(
        not(any(feature = "native-runtime", test)),
        expect(dead_code, reason = "only the files count a record at a time")
    )]
    pub fn step(&self) {
        self.done.fetch_add(1, Ordering::Relaxed);
    }

    /// So many read, or moved, by now.
    #[cfg_attr(
        not(any(feature = "web", test)),
        expect(
            dead_code,
            reason = "only a browser's database counts a page at a time"
        )
    )]
    pub fn so_far(&self, done: u64) {
        self.done.store(done, Ordering::Relaxed);
    }

    /// How far it has come, now.
    pub fn now(&self) -> HistoryProgress {
        let stage = self.stage.load(Ordering::Acquire);
        let (done, of) = (
            self.done.load(Ordering::Relaxed),
            self.of.load(Ordering::Relaxed),
        );
        HistoryProgress {
            moving: stage == MOVING,
            count: (stage != OPENING && of != UNCOUNTED).then(|| (done.min(of), of)),
        }
    }
}

/// How far the history read as the app opens has come, as the boot screen
/// says it ([`crate::persist::Store::history_progress`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryProgress {
    /// It is being moved from where an older build kept it whole, rather
    /// than read.
    pub moving: bool,
    /// So many records of so many, where they are counted.
    pub count: Option<(u64, u64)>,
}

impl HistoryProgress {
    /// In words for the boot screen: "Reading your history… 120 of 3000",
    /// or "Moving your history… 120 of 3000" while it is moved; with no
    /// count where none is kept.
    pub fn said(self) -> String {
        let doing = if self.moving { "Moving" } else { "Reading" };
        match self.count {
            Some((done, of)) => format!("{doing} your history… {done} of {of}"),
            None => format!("{doing} your history…"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The boot screen says how far the read, or the move, has come, so
    /// many of so many; and without a count where none is kept: before the
    /// records are listed, and while a move made whole at once is made.
    #[test]
    fn the_boot_screen_says_how_far_the_history_has_got() {
        let progress = LoadProgress::default();
        assert_eq!(progress.now().said(), "Reading your history…");
        progress.moving(Some(40));
        for _ in 0..12 {
            progress.step();
        }
        assert_eq!(progress.now().said(), "Moving your history… 12 of 40");
        progress.reading(Some(3000));
        progress.step();
        assert_eq!(progress.now().said(), "Reading your history… 1 of 3000");
        progress.so_far(2000);
        assert_eq!(
            progress.now(),
            HistoryProgress {
                moving: false,
                count: Some((2000, 3000)),
            }
        );
        // Never past what there is: a record stored meanwhile is read too.
        progress.so_far(3001);
        assert_eq!(progress.now().said(), "Reading your history… 3000 of 3000");
        progress.moving(None);
        assert_eq!(progress.now().said(), "Moving your history…");
        progress.reading(None);
        progress.so_far(500);
        assert_eq!(progress.now().said(), "Reading your history…");
    }
}
