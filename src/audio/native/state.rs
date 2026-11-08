//! The player's bookkeeping, with no device attached.
//!
//! The epoch keeps an old waiter from claiming a new send's ending; stopping
//! the sound itself is the mixer's business, send by send.

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use cw_core::TrainingSettings;
use cw_core::band::{BandChange, BandKey};

use crate::audio::epoch::{Epoch, Ticket};
use crate::audio::mixer::clock_sec;
use crate::audio::wait::LATE_MS;
use crate::audio::{PlaybackSignal, SendTiming, WaitFlags};

pub struct PlayerState {
    epoch: Epoch,
    /// What the band playing now was built from.
    band: Option<BandKey>,
}

impl PlayerState {
    pub fn new() -> Self {
        Self {
            epoch: Epoch::default(),
            band: None,
        }
    }

    /// Whether the band has to be retuned or rebuilt for these settings.
    /// The fading is not part of it: it is applied to the Morse, a send at a
    /// time.
    pub fn band_change(&self, settings: &TrainingSettings) -> BandChange {
        BandKey::of(&settings.band).change_from(self.band.as_ref())
    }

    pub fn note_band(&mut self, settings: &TrainingSettings) {
        self.band = Some(BandKey::of(&settings.band));
    }

    pub fn forget_band(&mut self) {
        self.band = None;
    }

    /// Claim the audio for a new send.
    pub fn arm(&mut self) -> Ticket {
        self.epoch.arm()
    }

    /// Retire the current send, stopped or never started, so nothing can
    /// mistake it for the current one.
    pub fn retire(&mut self) {
        self.epoch.bump();
    }
}

/// How long an open stream may go uncalled before it is taken for broken:
/// longer than the longest buffer a host's default gives, a third of a
/// second or so, with room for a busy machine.
pub const SILENT_AFTER_SEC: f64 = 1.5;

/// How long a stream just opened may go before it is called the first
/// time: a Bluetooth headset, or a USB sound card woken from saving power,
/// takes seconds to start playing, and a stream taken for broken meanwhile
/// is only opened again on a device as slow to start.
pub const FIRST_CALL_SEC: f64 = 5.0;

/// How long a send waiting on a stream sees it uncalled before it says the
/// stream broke: a process the system froze and has just let go is called
/// again within a buffer or two, and a stream that stopped stays stopped.
const SILENT_CONFIRM_SEC: f64 = 0.25;

/// When a stream's callback last ran, in seconds on [`clock_sec`]'s clock:
/// what tells a stream stopped without a word, as an iOS interruption stops
/// one, from one that plays. Opened, it is given [`FIRST_CALL_SEC`] to be
/// called the first time, unless its worker errs over and over first, and
/// [`SILENT_AFTER_SEC`] each time after.
pub struct Heartbeat {
    /// When the callback last ran, or, till it has, the stream opened.
    last: AtomicU64,
    /// Whether the callback has run at all.
    called: AtomicBool,
    /// Whether its worker has reported one error after another, no buffer
    /// asked for between them, since the callback last ran, or the stream
    /// opened: a worker in a loop of them.
    looping: AtomicBool,
}

impl Heartbeat {
    pub fn new(now_sec: f64) -> Self {
        Self {
            last: AtomicU64::new(now_sec.to_bits()),
            called: AtomicBool::new(false),
            looping: AtomicBool::new(false),
        }
    }

    /// The callback ran at `now_sec`.
    pub fn beat(&self, now_sec: f64) {
        self.last.store(now_sec.to_bits(), Ordering::Relaxed);
        if !self.called.load(Ordering::Relaxed) {
            self.called.store(true, Ordering::Release);
        }
        if self.looping.load(Ordering::Relaxed) {
            self.looping.store(false, Ordering::Release);
        }
    }

    /// When the callback last ran, or the stream opened.
    #[cfg(test)]
    pub fn last(&self) -> f64 {
        f64::from_bits(self.last.load(Ordering::Relaxed))
    }

    /// Whether the callback has run at all since the stream opened.
    pub fn called(&self) -> bool {
        self.called.load(Ordering::Acquire)
    }

    /// Its worker reported an error after another, with no buffer asked
    /// for between them: a worker in a loop of them, until the callback next
    /// runs. Before the callback has ever run, the device will not start,
    /// not one slow to, and the stream is given no longer to be called the
    /// first time than one that stopped is to be called again.
    pub fn erred_again(&self) {
        self.looping.store(true, Ordering::Release);
    }

    /// Whether its worker is in a loop of errors ([`Self::erred_again`]).
    pub fn looping(&self) -> bool {
        self.looping.load(Ordering::Acquire)
    }

    /// Whether, at `now_sec`, the callback has gone uncalled for longer than
    /// it is given, however long since anything last looked: longer than
    /// [`SILENT_AFTER_SEC`] since it was last called, or, never called,
    /// than [`FIRST_CALL_SEC`] since the stream opened, or than
    /// [`SILENT_AFTER_SEC`] where its worker has erred over and over.
    pub fn went_silent(&self, now_sec: f64) -> bool {
        let given = if self.called() || self.looping() {
            SILENT_AFTER_SEC
        } else {
            FIRST_CALL_SEC
        };
        now_sec - f64::from_bits(self.last.load(Ordering::Relaxed)) > given
    }

    /// Whether, at `now_sec`, the callback has gone uncalled for longer than
    /// [`SILENT_AFTER_SEC`], called before or not: a worker that has not
    /// called back in that time may be stuck in a call to its device, which
    /// letting go of the stream would wait on. A stream slow to start is
    /// given longer to be called the first time before it is taken for
    /// broken ([`Self::went_silent`]), not before it is let go of as one
    /// that may be stuck.
    pub fn may_be_stuck(&self, now_sec: f64) -> bool {
        now_sec - f64::from_bits(self.last.load(Ordering::Relaxed)) > SILENT_AFTER_SEC
    }
}

/// Whether a stream opened on the device named `opened` should move to the
/// default now named `default`: only when both names are known and differ,
/// so a device that cannot say its name never has the stream reopened over
/// and over.
pub fn moved_off_default(opened: Option<&str>, default: Option<&str>) -> bool {
    matches!((opened, default), (Some(opened), Some(default)) if opened != default)
}

/// What the stream tells a waiter.
pub struct ToneSignal {
    ticket: Ticket,
    finished: Arc<AtomicBool>,
    /// The send's word that it played late ([`crate::audio::WaitFlags::late`]).
    late: Arc<AtomicBool>,
    /// The word of the stream the send was started on that it broke. A send
    /// is never moved to another stream: one that replaces a broken stream
    /// may run at another rate.
    stream_failed: Arc<AtomicBool>,
    /// That stream's heartbeat, and since when this waiter has seen it gone
    /// silent: a stream stopped without a word is told of a moment after it
    /// stopped, not at the end of the stall the send is allowed.
    heartbeat: Arc<Heartbeat>,
    silent_since: Cell<Option<f64>>,
    /// When the send was handed to the stream, on [`clock_sec`]'s clock: a
    /// stream not yet called the first time holds it up from then.
    started_sec: f64,
    /// How long it waits to be rendered, its first sample going to the
    /// output, and how far behind the stream's callback the speaker was as
    /// it started.
    timing: SendTiming,
}

impl ToneSignal {
    pub fn new(
        ticket: Ticket,
        finished: Arc<AtomicBool>,
        late: Arc<AtomicBool>,
        stream_failed: Arc<AtomicBool>,
        heartbeat: Arc<Heartbeat>,
        started_sec: f64,
        timing: SendTiming,
    ) -> Self {
        Self {
            ticket,
            finished,
            late,
            stream_failed,
            heartbeat,
            silent_since: Cell::new(None),
            started_sec,
            timing,
        }
    }

    /// Whether the stream has broken by `now_sec`: said so, or seen silent
    /// for [`SILENT_CONFIRM_SEC`] on end, which it then says for the player
    /// to open another.
    fn stream_broke_at(&self, now_sec: f64) -> bool {
        if self.stream_failed.load(Ordering::Acquire) {
            return true;
        }
        if !self.heartbeat.went_silent(now_sec) {
            self.silent_since.set(None);
            return false;
        }
        let since = self.silent_since.get().unwrap_or(now_sec);
        self.silent_since.set(Some(since));
        let broke = now_sec - since >= SILENT_CONFIRM_SEC;
        if broke {
            self.stream_failed.store(true, Ordering::Release);
        }
        broke
    }

    /// Whether, at `now_sec`, the send has waited on a stream not yet
    /// called the first time for longer than a send may fall behind and
    /// still be timed ([`LATE_MS`]): held up while its device starts.
    fn held_by_start_at(&self, now_sec: f64) -> bool {
        !self.heartbeat.called() && now_sec - self.started_sec > LATE_MS as f64 / 1_000.0
    }

    /// What the stream tells the waiter at `now_sec`, on [`clock_sec`]'s
    /// clock.
    fn poll_at(&self, now_sec: f64) -> WaitFlags {
        WaitFlags {
            cancelled: self.ticket.is_cancelled(),
            finished: self.finished.load(Ordering::Acquire),
            // A broken stream never calls back again, so the send would never
            // finish; saying so lets the session rebuild the player.
            failed: self.stream_broke_at(now_sec),
            // A stream not yet called the first time is starting, not
            // stalled, as a Bluetooth headset or a USB sound card woken from
            // saving power takes seconds to: the send waits for it as long
            // as its heartbeat gives it ([`FIRST_CALL_SEC`]), which says when
            // it broke instead, and none of that wait counts against the
            // stall the send is allowed. Parked only once the wait has made
            // it late: a stream called a moment after the send was handed to
            // it has held nothing up. Nothing else parks a stream the way a
            // browser parks a hidden tab: it calls back or it does not.
            suspended: self.held_by_start_at(now_sec),
            // cpal gives no clock of its own: the send having been heard to
            // its end is the only word on when it is over, and the send says
            // itself whether it played late.
            played_ms: None,
            late: self.late.load(Ordering::Acquire),
            // A native stream plays everything it is handed, late if it
            // must, and skips nothing.
            missed: None,
        }
    }
}

impl PlaybackSignal for ToneSignal {
    fn timing(&self) -> SendTiming {
        self.timing
    }

    fn poll(&self) -> WaitFlags {
        self.poll_at(clock_sec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    use cw_core::{MonoMs, PlannedTransmission};

    use crate::audio::wait::Waited;
    use crate::audio::{PlaybackOutcome, PlaybackWait};
    use crate::time::{mono_ms, sleep_ms};

    #[test]
    fn a_stream_moves_only_when_the_default_does() {
        assert!(!moved_off_default(Some("Speakers"), Some("Speakers")));
        assert!(moved_off_default(Some("Speakers"), Some("Headphones")));
        assert!(!moved_off_default(None, Some("Headphones")));
        assert!(!moved_off_default(Some("Speakers"), None));
    }

    /// A stream is taken for broken once it has not been called for longer
    /// than a host's longest buffer and then some, counted from when it was
    /// last called, however long since anything looked: one that stopped
    /// minutes ago is known for stopped at the first look after.
    #[test]
    fn a_stream_left_uncalled_has_gone_silent() {
        let heartbeat = Heartbeat::new(10.0);
        heartbeat.beat(10.0);
        assert!(!heartbeat.went_silent(11.0));
        assert!(heartbeat.went_silent(11.6), "not called since it opened");
        heartbeat.beat(11.7);
        assert!(!heartbeat.went_silent(13.0));
        assert!(heartbeat.went_silent(13.3));
        assert!(heartbeat.went_silent(300.0), "stopped long ago");
        heartbeat.beat(299.99);
        assert!(!heartbeat.went_silent(300.0), "called all along");
    }

    /// A stream just opened is given longer to be called the first time
    /// than one called since is given to be called again: a Bluetooth
    /// headset, or a USB sound card woken from saving power, takes seconds
    /// to start, and each stream opened to try again on it takes as long.
    /// Taken for broken in a second and a half, every try of a send failed.
    #[test]
    fn a_stream_opened_is_given_longer_to_start_than_to_go_on() {
        let heartbeat = Heartbeat::new(10.0);
        assert!(!heartbeat.went_silent(13.0), "a device waking");
        assert!(!heartbeat.went_silent(10.0 + FIRST_CALL_SEC));
        assert!(heartbeat.went_silent(10.1 + FIRST_CALL_SEC), "never called");
        heartbeat.beat(13.0);
        assert!(!heartbeat.went_silent(14.0));
        assert!(heartbeat.went_silent(14.6), "called, then stopped");
    }

    /// A stream given time to start is not taken for broken meanwhile, but
    /// its worker may be stuck as soon as one that stopped would be: one
    /// that has not called back for that long, called before or not, may
    /// never come back to be told to stop.
    #[test]
    fn a_stream_given_time_to_start_may_be_stuck_as_soon_as_one_that_stopped() {
        let heartbeat = Heartbeat::new(10.0);
        assert!(!heartbeat.may_be_stuck(10.0 + SILENT_AFTER_SEC));
        let past = 10.1 + SILENT_AFTER_SEC;
        assert!(heartbeat.may_be_stuck(past), "never called");
        assert!(
            !heartbeat.went_silent(past),
            "and still given time to start"
        );
        heartbeat.beat(13.0);
        assert!(!heartbeat.may_be_stuck(14.0));
        assert!(heartbeat.may_be_stuck(14.6), "called, then stopped");
        assert!(heartbeat.went_silent(14.6));
    }

    fn waiter(heartbeat: &Arc<Heartbeat>) -> ToneSignal {
        ToneSignal::new(
            Epoch::default().arm(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            Arc::clone(heartbeat),
            0.0,
            SendTiming::default(),
        )
    }

    /// A send waiting on a stream that stops calling is told it broke a
    /// moment after the stream is seen silent, and says so for the player;
    /// one called again in that moment, as a process let go after the
    /// system froze it is, never.
    #[test]
    fn a_waiter_is_told_of_a_stream_stopped_without_a_word() {
        let heartbeat = Arc::new(Heartbeat::new(0.0));
        heartbeat.beat(0.0);
        let send = waiter(&heartbeat);
        assert!(!send.stream_broke_at(1.0));
        assert!(!send.stream_broke_at(1.6), "seen silent, not yet confirmed");
        assert!(send.stream_broke_at(1.6 + SILENT_CONFIRM_SEC));
        assert!(send.stream_failed.load(Ordering::Acquire), "and said so");
        let thawed = waiter(&Arc::new(Heartbeat::new(0.0)));
        thawed.heartbeat.beat(0.0);
        assert!(!thawed.stream_broke_at(2.0));
        thawed.heartbeat.beat(2.1);
        assert!(!thawed.stream_broke_at(2.1 + SILENT_CONFIRM_SEC));
        assert!(!thawed.stream_broke_at(3.0));
    }

    /// The test's clock, which its timers run on, in seconds: read in place
    /// of [`clock_sec`]'s by a signal polled through [`OnTestClock`] and by
    /// the stream it waits on.
    fn test_sec() -> f64 {
        mono_ms() as f64 / 1_000.0
    }

    /// A signal polled on the test's clock, so that its wait, the stall the
    /// send is allowed and its stream's heartbeat all go by the one clock.
    struct OnTestClock(ToneSignal);

    impl PlaybackSignal for OnTestClock {
        fn poll(&self) -> WaitFlags {
            self.0.poll_at(test_sec())
        }

        fn timing(&self) -> SendTiming {
            self.0.timing()
        }
    }

    /// A send `duration_sec` long on `heartbeat`'s stream, handed to it now
    /// on the test's clock, and its word that it has been heard out.
    fn send_on(heartbeat: &Arc<Heartbeat>, duration_sec: f64) -> (PlaybackWait, Arc<AtomicBool>) {
        let finished = Arc::new(AtomicBool::new(false));
        let signal = ToneSignal::new(
            Epoch::default().arm(),
            Arc::clone(&finished),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            Arc::clone(heartbeat),
            test_sec(),
            SendTiming::default(),
        );
        let plan = PlannedTransmission {
            wanted: cw_core::PlaybackPlan {
                events: Vec::new(),
                chars: Vec::new(),
                duration_sec,
                resolved_char_wpm: 20.0,
                resolved_effective_wpm: 18.0,
                fade_from_sec: 0.0,
            },
            others: Vec::new(),
            dry_wanted: false,
            muted_until: None,
        };
        let signal = Rc::new(OnTestClock(signal));
        (
            PlaybackWait::new(&plan, signal, MonoMs(mono_ms())),
            finished,
        )
    }

    /// `heartbeat`'s stream, called the first time `after_ms` from now on
    /// the test's clock and on from then until it has played `play_sec`,
    /// when the send on it, `finished`, has been heard out.
    async fn called_after(
        heartbeat: &Heartbeat,
        after_ms: u32,
        play_sec: f64,
        finished: &AtomicBool,
    ) {
        sleep_ms(after_ms).await;
        let first = test_sec();
        while test_sec() - first < play_sec {
            heartbeat.beat(test_sec());
            sleep_ms(5).await;
        }
        heartbeat.beat(test_sec());
        finished.store(true, Ordering::Release);
    }

    /// A send on a stream its device is slow to call the first time, as a
    /// Bluetooth headset or a USB sound card woken from saving power is,
    /// waits for it as long as the stream is given to start: a second's
    /// send on a stream first called three seconds on is heard out, held up
    /// on its way, and one on a stream called a moment on is not held up at
    /// all. One on a stream never called fails as its heartbeat gives the
    /// stream up, and not before. That wait used to count against the
    /// stall a send is allowed, and every try failed two seconds past the
    /// send's length, the stream opened for each as slow to start.
    #[test]
    fn a_send_waits_for_a_stream_slow_to_start_as_long_as_the_stream_is_given() {
        crate::testing::run(|| async {
            let slow = Arc::new(Heartbeat::new(test_sec()));
            let (wait, finished) = send_on(&slow, 1.0);
            let (waited, ()) =
                tokio::join!(wait.wait(), called_after(&slow, 3_000, 1.0, &finished));
            assert_eq!(
                waited,
                Waited {
                    outcome: PlaybackOutcome::Completed,
                    held_up: true,
                },
                "a stream first called three seconds on"
            );

            let prompt = Arc::new(Heartbeat::new(test_sec()));
            let (wait, finished) = send_on(&prompt, 1.0);
            let (waited, ()) = tokio::join!(wait.wait(), called_after(&prompt, 20, 1.0, &finished));
            assert_eq!(
                waited,
                Waited {
                    outcome: PlaybackOutcome::Completed,
                    held_up: false,
                },
                "a stream called a moment on"
            );

            let never = Arc::new(Heartbeat::new(test_sec()));
            let (wait, _) = send_on(&never, 0.0);
            let began = test_sec();
            let waited = wait.wait().await;
            let took = test_sec() - began;
            assert_eq!(waited.outcome, PlaybackOutcome::Failed);
            let given = FIRST_CALL_SEC + SILENT_CONFIRM_SEC;
            assert!(
                (given..given + 0.05).contains(&took),
                "a stream never called failed {took} s in"
            );
        });
    }

    #[test]
    fn the_background_is_rebuilt_only_when_its_settings_change() {
        let mut state = PlayerState::new();
        let settings = TrainingSettings::default();
        assert_eq!(state.band_change(&settings), BandChange::Rebuild);
        state.note_band(&settings);
        assert_eq!(state.band_change(&settings), BandChange::None);

        let mut louder = settings.clone();
        louder.band.qrn_level = 0.9;
        assert_eq!(state.band_change(&louder), BandChange::Rebuild);

        // The receiver's controls retune the band on the air.
        let mut narrower = settings.clone();
        narrower.band.filter_width_hz = 250.0;
        assert_eq!(state.band_change(&narrower), BandChange::Retune);

        // Speed is not a band setting.
        let mut faster = settings.clone();
        faster.playback.char_wpm_min = 40.0;
        assert_eq!(state.band_change(&faster), BandChange::None);

        // Nor is the fading: it plays on the Morse, a send at a time, and a
        // new band would only be a new draw of stations and static for
        // nothing.
        let mut fading = settings.clone();
        fading.band.qsb_depth = 0.9;
        fading.band.qsb_rate_hz = 0.5;
        assert_eq!(state.band_change(&fading), BandChange::None);
        fading.band.qsb_enabled = false;
        assert_eq!(state.band_change(&fading), BandChange::None);

        state.forget_band();
        assert_eq!(state.band_change(&settings), BandChange::Rebuild);
    }

    #[test]
    fn the_signal_reports_what_the_stream_is_doing() {
        let mut state = PlayerState::new();
        let armed = state.arm();
        let finished = Arc::new(AtomicBool::new(false));
        let broken = Arc::new(AtomicBool::new(false));
        let late = Arc::new(AtomicBool::new(false));
        let timing = SendTiming {
            lead_sec: 0.0,
            latency_sec: 0.04,
        };
        // A stream that plays.
        let heartbeat = Arc::new(Heartbeat::new(clock_sec()));
        heartbeat.beat(clock_sec());
        let signal = ToneSignal::new(
            armed,
            Arc::clone(&finished),
            Arc::clone(&late),
            Arc::clone(&broken),
            heartbeat,
            clock_sec(),
            timing,
        );
        assert_eq!(signal.timing().heard_after_sec(), 0.04);
        assert_eq!(
            signal.poll(),
            WaitFlags {
                cancelled: false,
                finished: false,
                failed: false,
                suspended: false,
                played_ms: None,
                late: false,
                missed: None,
            }
        );

        late.store(true, Ordering::Release);
        assert!(signal.poll().late, "the send's word that it played late");
        finished.store(true, Ordering::Release);
        assert!(signal.poll().finished);

        broken.store(true, Ordering::Release);
        assert!(
            signal.poll().failed,
            "a broken device has to reach the waiter"
        );

        state.retire();
        assert!(signal.poll().cancelled);
    }
}
