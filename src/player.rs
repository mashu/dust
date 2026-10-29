//! The audio player, and whose turn it is to use it.
//!
//! One player sounds at a time, and several things want it: a training
//! session, the band preview, the letter player, an envelope sample. Whoever
//! claims it is handed a [`Lease`], and claiming it again — or giving it back
//! — ends every lease before. A lease is all a task needs to know whether the
//! audio is still its own: every send and every wait goes through one, and a
//! lease that has ended can neither send nor wake up as though it had not.
//!
//! This is also the only place the app talks to a [`MorseBackend`].

use std::cell::{Cell, RefCell, RefMut};
use std::fmt;
use std::rc::Rc;

use cw_core::{MonoMs, PlannedTransmission, SendStart, ToneEdge, TrainingSettings, Transmission};

use crate::audio::{MorseBackend, PlaybackOutcome, PlaybackWait, ToneRequest};
use crate::time::{POLL_MS, mono_ms, mono_secs_since, sleep_ms};

/// A factory for the audio backend. Injectable so the app can be driven
/// without a sound card.
pub type BackendFactory = Rc<dyn Fn() -> Result<Box<dyn MorseBackend>, String>>;

/// How many times one send is tried before the caller is told the audio is
/// gone. Each retry rebuilds the player, which is what picks up a device that
/// was swapped, unplugged or suspended mid-send.
const SEND_ATTEMPTS: u32 = 3;

/// How long the audio is quiet before the output is let sleep: long after
/// the last fade has played out, and long enough that a screen left and
/// come back to, or the next squeeze of a paddle, does not wake it.
const SLEEP_AFTER_MS: u64 = 1_000;

/// Why audio did not happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioError {
    /// The lease ended before the work did: something else took the audio,
    /// or the send was stopped. Nothing went wrong, so nothing is shown.
    Cancelled,
    /// The device would not open, or would not take the send. Its own words.
    Device(String),
    /// A send started and never reached its end: a stream that stopped
    /// calling back, an audio clock that stopped moving.
    Stalled,
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("Audio was stopped."),
            Self::Device(message) => f.write_str(message),
            Self::Stalled => f.write_str("Audio playback stalled."),
        }
    }
}

/// A send that played to its end: how long it was, and the speeds it went
/// at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sent {
    pub duration_sec: f64,
    pub char_wpm: f64,
    pub effective_wpm: f64,
}

/// What a send says of itself as it goes, to whoever sent it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SendNews<'a> {
    /// An attempt at it started ([`SendStart`]): each attempt says so.
    Started(&'a SendStart),
    /// Its audio was held up on its way, parked or its clock fallen behind:
    /// it went out later than planned, and none of it can be timed. Said
    /// once known, as it ends or is stopped.
    HeldUp,
}

/// The one audio player the app has. Cheap to clone; every clone is the same
/// player.
#[derive(Clone)]
pub struct Player(Rc<Shared>);

struct Shared {
    backend: RefCell<Option<Box<dyn MorseBackend>>>,
    make: BackendFactory,
    /// Moves on every claim and every release. A lease is current while the
    /// turn is still the one it was handed.
    turn: Cell<u64>,
    /// Moves whenever the send in flight is stopped, or a newer one is asked
    /// for, so a send that has not started yet — one waiting out a retry —
    /// can tell it is no longer wanted.
    sends: Cell<u64>,
    /// When the audio was last claimed, given back or keyed: it has been
    /// quiet since.
    stirred_at: Cell<u64>,
    /// A lease is out, and has not been given back.
    held: Cell<bool>,
    /// The paddle's sidetone is sounding.
    keyed: Cell<bool>,
    /// A screen has the output kept awake for the first squeeze.
    kept_ready: Cell<bool>,
    /// A wait to let the output sleep is under way.
    sleep_waiting: Cell<bool>,
}

impl Player {
    pub fn new(make: BackendFactory) -> Self {
        Self(Rc::new(Shared {
            backend: RefCell::new(None),
            make,
            turn: Cell::new(0),
            sends: Cell::new(0),
            stirred_at: Cell::new(0),
            held: Cell::new(false),
            keyed: Cell::new(false),
            kept_ready: Cell::new(false),
            sleep_waiting: Cell::new(false),
        }))
    }

    /// Take the audio. Whatever had it stops, the player is opened for the
    /// settings, and the lease that comes back is the new holder's.
    ///
    /// Whatever had it is ended even when the player cannot be opened, so a
    /// failed claim leaves nothing sounding and nothing holding the audio.
    pub fn claim(&self, settings: &TrainingSettings) -> Result<Lease, AudioError> {
        self.stop_sending();
        let turn = self.next_turn();
        self.stir(true);
        match self.open(settings) {
            Ok(()) => Ok(Lease {
                player: self.clone(),
                turn,
            }),
            Err(err) => {
                self.with_backend(|backend| backend.shutdown());
                self.stir(false);
                Err(err)
            }
        }
    }

    /// Give the audio back: every lease ends, and nothing sounds, the
    /// receiver included. What "stop" means when the user pressed it, or
    /// walked away from the screen that was making the sound. Once it has
    /// been quiet a while, the output sleeps.
    pub fn release(&self) {
        self.next_turn();
        self.with_backend(|backend| backend.shutdown());
        self.stir(false);
    }

    /// Keep the output awake for the paddle's first squeeze, `ready`, as
    /// the screen the paddles are tried on does; or let it sleep once
    /// quiet, as everywhere else.
    pub fn keep_ready(&self, ready: bool) {
        self.0.kept_ready.set(ready);
        if !ready {
            self.sleep_once_quiet();
        }
    }

    /// Something claimed the audio, `held`, or gave it back or keyed it: it
    /// is quiet from now, and the output sleeps once it has been a while.
    fn stir(&self, held: bool) {
        self.0.held.set(held);
        self.0.stirred_at.set(mono_ms());
        self.sleep_once_quiet();
    }

    /// Let the output sleep once nothing has claimed, given back or keyed
    /// the audio for [`SLEEP_AFTER_MS`], unless a lease holds it, the
    /// sidetone sounds, or a screen keeps it ready. One wait at a time,
    /// which reads the latest stir. Without the app's runtime, as in a test
    /// of the player alone, there is nothing to wait with, and it never
    /// sleeps.
    fn sleep_once_quiet(&self) {
        let shared = &self.0;
        if shared.held.get()
            || shared.keyed.get()
            || shared.kept_ready.get()
            || shared.sleep_waiting.get()
            || dioxus::core::Runtime::try_current().is_none()
        {
            return;
        }
        shared.sleep_waiting.set(true);
        let player = self.clone();
        dioxus::core::spawn_forever(async move {
            let shared = &player.0;
            loop {
                let due = shared.stirred_at.get() + SLEEP_AFTER_MS;
                let now = mono_ms();
                if now >= due {
                    break;
                }
                sleep_ms(u32::try_from(due - now).unwrap_or(u32::MAX)).await;
            }
            shared.sleep_waiting.set(false);
            // Whatever has woken it meanwhile waits on in turn when it is
            // done.
            if !(shared.held.get() || shared.keyed.get() || shared.kept_ready.get()) {
                player.with_backend(|backend| backend.sleep());
            }
        });
    }

    /// Bring the band in line with settings that have just changed, while a
    /// preview of it is playing.
    pub fn follow_band(&self, settings: &TrainingSettings) {
        // A slider move has no one to tell. A band that cannot open is
        // reported by the next send, which rebuilds the player.
        self.with_backend(|backend| backend.apply_band(settings));
    }

    /// Gate the live sidetone for paddle practice, at the edge the keyer
    /// timed it for. Opens the player if there is none, so a squeeze
    /// outside training still has somewhere to go.
    pub fn set_live_tone(&self, edge: ToneEdge, settings: &TrainingSettings) {
        // A key that cannot sound has nowhere to report it mid-squeeze;
        // `ensure_live_sidetone` is where the reason is surfaced.
        let tone = ToneRequest::keyed(edge.on, settings, mono_secs_since(edge.at));
        let _ = self.gate_live_tone(tone);
    }

    /// Open the silent sidetone stream, so the first paddle squeeze is one
    /// audio buffer of lag rather than a stream start. Says why when it
    /// cannot.
    pub fn ensure_live_sidetone(&self, settings: &TrainingSettings) -> Result<(), AudioError> {
        self.gate_live_tone(ToneRequest::keyed(false, settings, 0.0))
    }

    fn gate_live_tone(&self, tone: ToneRequest) -> Result<(), AudioError> {
        {
            let mut backend = self.opened()?;
            backend.resume_from_gesture();
            backend.set_live_tone(tone);
        }
        self.0.keyed.set(tone.on);
        self.stir(self.0.held.get());
        Ok(())
    }

    fn next_turn(&self) -> u64 {
        let turn = self.0.turn.get() + 1;
        self.0.turn.set(turn);
        turn
    }

    /// Stop the send in flight and anything waiting to start one, leaving
    /// the receiver running. Between groups the background is meant to keep
    /// hissing: a real receiver does not go silent because the other station
    /// stopped keying.
    fn stop_sending(&self) {
        self.0.sends.set(self.0.sends.get() + 1);
        self.with_backend(|backend| backend.stop());
    }

    /// The backend, opened first if there is none.
    fn opened(&self) -> Result<RefMut<'_, Box<dyn MorseBackend>>, AudioError> {
        let mut slot = self.0.backend.borrow_mut();
        if slot.is_none() {
            *slot = Some((self.0.make)().map_err(AudioError::Device)?);
        }
        RefMut::filter_map(slot, Option::as_mut).map_err(|_| unavailable())
    }

    /// Open the player if there is none, and set it up for `settings`, the
    /// sidetone silent.
    fn open(&self, settings: &TrainingSettings) -> Result<(), AudioError> {
        let mut backend = self.opened()?;
        backend.resume_from_gesture();
        backend.apply_band(settings);
        backend.set_live_tone(ToneRequest::keyed(false, settings, 0.0));
        self.0.keyed.set(false);
        Ok(())
    }

    /// Throw the player away and open a fresh one.
    fn rebuild(&self, settings: &TrainingSettings) -> Result<(), AudioError> {
        self.discard();
        self.open(settings)
    }

    /// Throw the player away, silenced: the next claim or send opens a
    /// fresh one.
    fn discard(&self) {
        // Taken out first, so the borrow is over before anything else takes
        // its own.
        let old = self.0.backend.borrow_mut().take();
        if let Some(mut old) = old {
            old.shutdown();
        }
    }

    /// The way in to the backend, when there is one. Every borrow is
    /// synchronous, so a borrow that is already out is a bug, and panics
    /// rather than being skipped in silence.
    fn with_backend(&self, act: impl FnOnce(&mut dyn MorseBackend)) {
        if let Some(backend) = self.0.backend.borrow_mut().as_mut() {
            act(backend.as_mut());
        }
    }

    /// Start one send on the player as it is, the way `begin` starts it.
    fn start(&self, begin: &Begin<'_>) -> Result<PlaybackWait, AudioError> {
        let mut slot = self.0.backend.borrow_mut();
        let backend = slot.as_mut().ok_or_else(unavailable)?;
        begin(backend.as_mut()).map_err(AudioError::Device)
    }
}

/// How one attempt at a send is started on a backend: planned there, or
/// planned already.
type Begin<'a> = dyn Fn(&mut dyn MorseBackend) -> Result<PlaybackWait, String> + 'a;

fn unavailable() -> AudioError {
    AudioError::Device("Audio is unavailable.".into())
}

/// The audio, held: a claim nothing has superseded yet.
#[derive(Clone)]
pub struct Lease {
    player: Player,
    turn: u64,
}

impl Lease {
    /// Whether this lease still holds the audio.
    pub fn is_current(&self) -> bool {
        self.player.0.turn.get() == self.turn
    }

    /// Whether two leases are the same claim.
    pub fn is(&self, other: &Lease) -> bool {
        self.turn == other.turn && Rc::ptr_eq(&self.player.0, &other.player.0)
    }

    /// Which claim this is, in the order they were made.
    pub fn turn(&self) -> u64 {
        self.turn
    }

    /// Give the audio back if this lease still holds it. Says whether it did.
    pub fn release(&self) -> bool {
        let current = self.is_current();
        if current {
            self.player.release();
        }
        current
    }

    /// Stop the send in flight, if this lease still holds the audio. The
    /// receiver keeps running.
    pub fn stop_sending(&self) {
        if self.is_current() {
            self.player.stop_sending();
        }
    }

    /// Have the player ready to sound, from inside a key or a tap: a
    /// context the browser suspended is resumed, and a player that is gone
    /// is opened again. Nothing, once this lease has ended. A player that
    /// will not open says so on the next send, which tries again.
    pub fn wake(&self) {
        if !self.is_current() {
            return;
        }
        if let Ok(mut backend) = self.player.opened() {
            backend.resume_from_gesture();
        }
    }

    /// Ask for a send that a task will start later, ending every send asked
    /// for before it: one waiting out a retry gives up rather than start
    /// over the new one. What is sounding plays on until the new send
    /// starts in its place.
    ///
    /// The send is this one from now, not from when its task first runs,
    /// so a stop handled in between ends it too.
    pub fn ask_to_send(&self) -> Asked {
        let sends = &self.player.0.sends;
        // A lease that has ended leaves the holder's sends alone; its own
        // never start.
        if self.is_current() {
            sends.set(sends.get() + 1);
        }
        Asked(sends.get())
    }

    /// Wait `ms`, in one go, and say whether the lease outlived it.
    pub async fn sleep(&self, ms: u32) -> Result<(), AudioError> {
        sleep_ms(ms).await;
        self.check()
    }

    fn check(&self) -> Result<(), AudioError> {
        if self.is_current() {
            Ok(())
        } else {
            Err(AudioError::Cancelled)
        }
    }

    /// Send `transmission` and wait for it to be heard.
    ///
    /// A send that fails is tried again on a freshly built player, up to
    /// [`SEND_ATTEMPTS`] in all, and the last failure is the one reported.
    /// A send that is stopped — the lease ending, or [`Lease::stop_sending`]
    /// — comes back [`AudioError::Cancelled`] from wherever it had got to,
    /// a retry included. What it says of itself as it goes is told to
    /// `on_news`, as for [`Lease::send_planned`].
    pub async fn send(
        &self,
        transmission: &Transmission,
        settings: &TrainingSettings,
        mut on_news: impl FnMut(SendNews<'_>),
    ) -> Result<Sent, AudioError> {
        let begin = |backend: &mut dyn MorseBackend| {
            crate::audio::start_send(backend, transmission, settings)
        };
        // A session sends a group only once the one before is over or
        // stopped, so this is the send from when it starts.
        let asked = Asked(self.player.0.sends.get());
        self.send_with(asked, &begin, settings, &mut on_news).await
    }

    /// Send a contest's turn, planned already, and wait for it to be heard:
    /// as [`Lease::send`] does, retries and all, as the send `asked` for
    /// ([`Lease::ask_to_send`]). `transcript` is what the turn says, for a
    /// player that keeps it. `starts_at`, for a continuation, is when its
    /// sound is to reach the ear: each attempt asks the player for it, and
    /// one after it has passed, a retry, starts as soon as it can.
    ///
    /// Each attempt, once started, tells `on_news` so, with the send as it
    /// starts ([`SendStart`]): when it goes to the output and reaches the
    /// ear, and where its characters end; and a send held up on its way
    /// says that too ([`SendNews`]). No borrow of the player is held then,
    /// so `on_news` may use it.
    pub async fn send_planned(
        &self,
        asked: Asked,
        plan: &PlannedTransmission,
        transcript: &Transmission,
        settings: &TrainingSettings,
        starts_at: Option<MonoMs>,
        mut on_news: impl FnMut(SendNews<'_>),
    ) -> Result<Sent, AudioError> {
        let begin = |backend: &mut dyn MorseBackend| {
            crate::audio::start_planned(backend, plan, transcript, settings, starts_at)
        };
        self.send_with(asked, &begin, settings, &mut on_news).await
    }

    /// A send, each attempt started by `begin`: tried again on a freshly
    /// built player, up to [`SEND_ATTEMPTS`] in all. A player that failed
    /// every attempt is thrown away, so whatever claims the audio next — a
    /// tap on Resume, a new session — opens a fresh one, and opens it inside
    /// the tap, where a browser lets it start.
    async fn send_with(
        &self,
        asked: Asked,
        begin: &Begin<'_>,
        settings: &TrainingSettings,
        on_news: &mut dyn FnMut(SendNews<'_>),
    ) -> Result<Sent, AudioError> {
        let ticket = Ticket {
            lease: self,
            sends: asked.0,
        };
        let mut outcome = ticket.attempt(begin, on_news).await;
        for _ in 1..SEND_ATTEMPTS {
            if matches!(outcome, Ok(_) | Err(AudioError::Cancelled)) {
                break;
            }
            // A send stopped or replaced meanwhile is not tried again, and
            // leaves the player to the send that has it now.
            ticket.check()?;
            let rebuilt = self.player.rebuild(settings);
            sleep_ms(POLL_MS).await;
            ticket.check()?;
            outcome = match rebuilt {
                Ok(()) => ticket.attempt(begin, on_news).await,
                // There is no player to send through, and the reason it could
                // not be opened is the one worth showing.
                Err(err) => Err(err),
            };
        }
        if matches!(outcome, Err(AudioError::Device(_) | AudioError::Stalled)) {
            // A send stopped or replaced meanwhile has failed no one, and
            // the player is the newer send's now.
            ticket.check()?;
            self.player.discard();
        }
        outcome
    }
}

/// A send asked for ([`Lease::ask_to_send`]): which one it is, so a stop,
/// a claim, or a newer send asked for after it can tell it it is no longer
/// wanted.
#[derive(Clone, Copy, Debug)]
pub struct Asked(u64);

/// One send's right to the player: the lease it was made under, and which
/// send it is.
struct Ticket<'a> {
    lease: &'a Lease,
    sends: u64,
}

impl Ticket<'_> {
    fn check(&self) -> Result<(), AudioError> {
        self.lease.check()?;
        if self.lease.player.0.sends.get() == self.sends {
            Ok(())
        } else {
            Err(AudioError::Cancelled)
        }
    }

    async fn attempt(
        &self,
        begin: &Begin<'_>,
        on_news: &mut dyn FnMut(SendNews<'_>),
    ) -> Result<Sent, AudioError> {
        let player = &self.lease.player;
        self.check()?;
        // In a statement of its own: the player is borrowed only while the
        // send starts, never while `on_news` runs.
        let wait = player.start(begin)?;
        let start = wait.start();
        on_news(SendNews::Started(&start));
        let duration_sec = wait.duration_sec;
        let waited = wait.wait().await;
        // Heard out or stopped, a send held up on its way says so: what of
        // it was heard went out late. One that failed is sent again.
        if waited.held_up && waited.outcome != PlaybackOutcome::Failed {
            on_news(SendNews::HeldUp);
        }
        match waited.outcome {
            PlaybackOutcome::Completed => self.check().map(|()| Sent {
                duration_sec,
                char_wpm: start.char_wpm,
                effective_wpm: start.effective_wpm,
            }),
            PlaybackOutcome::Cancelled => Err(AudioError::Cancelled),
            // The send never reached its end: rebuilding the player on the
            // next attempt is what recovers it.
            PlaybackOutcome::Failed => Err(AudioError::Stalled),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::fake::{Behaviour, Call, Recorder};
    use crate::testing::{run, test_settings};

    fn player() -> (Player, Rc<Recorder>) {
        let recorder = Recorder::new();
        (Player::new(recorder.factory()), recorder)
    }

    fn cq() -> Transmission {
        let voice = cw_core::resolve_station(&test_settings(), &mut cw_core::SplitMix64::new(1));
        Transmission::alone("CQ", voice)
    }

    #[test]
    fn a_claim_ends_the_lease_before_it_and_keeps_the_player() {
        run(|| async {
            let (player, recorder) = player();
            let first = player.claim(&test_settings()).expect("a player");
            assert!(first.is_current());
            assert_eq!(recorder.calls().first(), Some(&Call::New));

            let second = player.claim(&test_settings()).expect("a player");
            assert!(!first.is_current() && second.is_current());
            assert!(!first.is(&second) && second.is(&second.clone()));
            assert!(recorder.calls().contains(&Call::Stop));
            // Taking over is not a rebuild.
            assert_eq!(recorder.players_built.get(), 1);

            // An ended lease cannot give back what it no longer holds.
            assert!(!first.release());
            assert!(second.is_current());
            assert!(second.release());
            assert!(!second.is_current());
            assert_eq!(recorder.calls().last(), Some(&Call::Shutdown));
        });
    }

    /// A claim that cannot open a player still ends whatever had the audio,
    /// and leaves nothing sounding behind it.
    #[test]
    fn a_failed_claim_still_ends_what_came_before() {
        run(|| async {
            let mut settings = test_settings();
            settings.band.noise_enabled = true;
            let (player, recorder) = player();
            let first = player.claim(&settings).expect("a player");
            assert!(recorder.band_running.get());

            // The player goes, and no new one can be had.
            player.0.backend.borrow_mut().take();
            recorder.build_error.set(true);
            assert_eq!(
                player.claim(&settings).err(),
                Some(AudioError::Device("No audio output device found".into()))
            );
            assert!(!first.is_current());
        });
    }

    #[test]
    fn nothing_to_stop_is_no_trouble() {
        let (player, recorder) = player();
        player.release();
        player.follow_band(&test_settings());
        assert!(recorder.calls().is_empty());
    }

    #[test]
    fn the_band_follows_the_settings_while_a_preview_runs() {
        run(|| async {
            let (player, recorder) = player();
            let mut settings = test_settings();
            let _lease = player.claim(&settings).expect("a player");
            recorder.clear();
            settings.band.qrn_enabled = true;
            settings.band.qrn_level = 0.4;
            player.follow_band(&settings);
            assert_eq!(
                recorder.calls(),
                vec![Call::Band(cw_core::band::BandKey::of(&settings.band))]
            );
        });
    }

    #[test]
    fn a_send_reports_how_it_was_sent() {
        run(|| async {
            let (player, recorder) = player();
            let lease = player.claim(&test_settings()).expect("a player");
            let sent = lease
                .send(&cq(), &test_settings(), |_| {})
                .await
                .expect("sent");
            assert!(sent.duration_sec > 0.0);
            assert_eq!(sent.char_wpm, 40.0);
            assert_eq!(recorder.texts(), vec!["CQ".to_string()]);
        });
    }

    #[test]
    fn a_send_under_an_ended_lease_never_starts() {
        run(|| async {
            let (player, recorder) = player();
            let lease = player.claim(&test_settings()).expect("a player");
            player.release();
            assert_eq!(
                lease.send(&cq(), &test_settings(), |_| {}).await,
                Err(AudioError::Cancelled)
            );
            assert!(recorder.texts().is_empty());
            assert_eq!(lease.sleep(10).await, Err(AudioError::Cancelled));
        });
    }

    /// The last failure is the one reported, after a fresh player for each
    /// retry.
    #[test]
    fn a_failing_device_is_retried_on_a_fresh_player_then_reported() {
        run(|| async {
            let (player, recorder) = player();
            let lease = player.claim(&test_settings()).expect("a player");
            recorder.set(Behaviour::FailMidSend);
            assert_eq!(
                lease.send(&cq(), &test_settings(), |_| {}).await,
                Err(AudioError::Stalled)
            );
            assert_eq!(recorder.texts().len(), SEND_ATTEMPTS as usize);
            assert_eq!(recorder.players_built.get(), SEND_ATTEMPTS);

            recorder.set(Behaviour::RefuseToStart);
            assert_eq!(
                lease.send(&cq(), &test_settings(), |_| {}).await,
                Err(AudioError::Device("Audio stream: device is gone".into()))
            );
        });
    }

    #[test]
    fn a_send_with_no_player_left_opens_one() {
        run(|| async {
            let (player, recorder) = player();
            let lease = player.claim(&test_settings()).expect("a player");
            // The player is gone, the way a failed rebuild leaves it.
            player.0.backend.borrow_mut().take();
            assert!(lease.send(&cq(), &test_settings(), |_| {}).await.is_ok());
            assert_eq!(recorder.players_built.get(), 2);
        });
    }

    /// A turn planned already plays as planned, and each attempt says when
    /// it started and how long its sound takes to reach the ear, retries
    /// too.
    #[test]
    fn a_planned_send_says_when_each_attempt_starts() {
        run(|| async {
            let (player, recorder) = player();
            let settings = test_settings();
            let lease = player.claim(&settings).expect("a player");
            recorder.latency_sec.set(0.25);
            let transmission = cq();
            let plan = cw_core::plan_transmission(&transmission, &settings);
            let mut starts = Vec::new();
            recorder.fail_next(1);
            let asked = lease.ask_to_send();
            let sent = lease
                .send_planned(
                    asked,
                    &plan,
                    &transmission,
                    &settings,
                    None,
                    |news: SendNews<'_>| {
                        if let SendNews::Started(start) = news {
                            starts.push(start.latency_ms);
                        }
                    },
                )
                .await;
            assert_eq!(
                sent.map(|sent| sent.duration_sec),
                Ok(plan.wanted.duration_sec)
            );
            assert_eq!(starts, [250, 250], "one call for each attempt");
            let kept = recorder.starts();
            assert_eq!(kept.len(), 2);
            assert_eq!(kept[0].transcript, transmission);
            assert_eq!(kept[0].plan.wanted.duration_sec, plan.wanted.duration_sec);
        });
    }

    /// A send asked to start at a given time is started then: the player
    /// is asked for that moment, its start says its sound is heard then, and
    /// the wait runs on from there to the end of the send. Asked for a
    /// moment already past, as a retry is, it starts as soon as it can.
    #[test]
    fn a_planned_send_starts_at_its_time() {
        run(|| async {
            let (player, recorder) = player();
            let settings = test_settings();
            let lease = player.claim(&settings).expect("a player");
            let transmission = cq();
            let plan = cw_core::plan_transmission(&transmission, &settings);
            let began = crate::time::mono_ms();
            let at = MonoMs(began + 400);
            let mut starts = Vec::new();
            let asked = lease.ask_to_send();
            let sent = lease
                .send_planned(
                    asked,
                    &plan,
                    &transmission,
                    &settings,
                    Some(at),
                    |news: SendNews<'_>| {
                        if let SendNews::Started(start) = news {
                            starts.push(start.heard_at());
                        }
                    },
                )
                .await;
            assert!(sent.is_ok());
            assert_eq!(starts, [at]);
            assert_eq!(recorder.starts()[0].heard_in, Some(0.4));
            let took = crate::time::mono_ms() - began;
            let planned = (plan.wanted.duration_sec * 1_000.0) as u64;
            assert!(took >= 400 + planned, "done {took} ms in, of {planned}");

            let now = MonoMs(crate::time::mono_ms());
            let past = MonoMs(now.0 - 200);
            let mut starts = Vec::new();
            let asked = lease.ask_to_send();
            let sent = lease
                .send_planned(
                    asked,
                    &plan,
                    &transmission,
                    &settings,
                    Some(past),
                    |news: SendNews<'_>| {
                        if let SendNews::Started(start) = news {
                            starts.push(start.heard_at());
                        }
                    },
                )
                .await;
            assert!(sent.is_ok());
            assert_eq!(starts, [now], "at once");
            assert_eq!(recorder.starts()[1].heard_in, Some(-0.2));
        });
    }

    /// A send says as it starts when it goes to the output and where its
    /// characters end after that, the delay the device says follows apart;
    /// and, heard out or stopped, whether it was held up on its way, its
    /// clock parked.
    #[test]
    fn a_send_says_its_timeline_as_it_starts_and_whether_it_was_held_up() {
        run(|| async {
            let (player, recorder) = player();
            let settings = test_settings();
            let lease = player.claim(&settings).expect("a player");
            recorder.latency_sec.set(0.25);
            let began = MonoMs(crate::time::mono_ms());
            let mut starts = Vec::new();
            let mut held_up = false;
            let sent = lease
                .send(&cq(), &settings, |news| match news {
                    SendNews::Started(start) => starts.push(start.clone()),
                    SendNews::HeldUp => held_up = true,
                })
                .await
                .expect("sent");
            let [start] = starts.as_slice() else {
                panic!("one start: {starts:?}");
            };
            assert_eq!(start.output_at, began);
            assert_eq!(start.latency_ms, 250);
            assert_eq!(start.heard_at(), began.plus(250));
            // The last character ends the send.
            assert_eq!(start.char_ends_ms.len(), 2, "{start:?}");
            assert_eq!(
                start.char_ends_ms.last().copied(),
                Some((sent.duration_sec * 1_000.0).round() as u32)
            );
            assert!(!held_up);

            // Parked on its way, heard out: held up.
            let parked = async {
                sleep_ms(100).await;
                recorder.set_page_hidden(true);
                sleep_ms(500).await;
                recorder.set_page_hidden(false);
            };
            let cq = cq();
            let mut news = Vec::new();
            let send = lease.send(&cq, &settings, |said| {
                news.push(matches!(said, SendNews::HeldUp));
            });
            let (sent, ()) = tokio::join!(send, parked);
            assert!(sent.is_ok());
            assert_eq!(news, [false, true], "started, then held up");

            // Parked on its way, and stopped there: held up all the same.
            let parked_then_stopped = async {
                sleep_ms(100).await;
                recorder.set_page_hidden(true);
                sleep_ms(300).await;
                lease.stop_sending();
            };
            let mut news = Vec::new();
            let send = lease.send(&cq, &settings, |said| {
                news.push(matches!(said, SendNews::HeldUp));
            });
            let (sent, ()) = tokio::join!(send, parked_then_stopped);
            recorder.set_page_hidden(false);
            assert_eq!(sent, Err(AudioError::Cancelled));
            assert_eq!(news, [false, true]);
        });
    }

    /// `on_news` may stop the send it is told about: nothing of the player
    /// is borrowed while it runs, and the send comes back cancelled.
    #[test]
    fn a_planned_send_can_be_stopped_as_it_starts() {
        run(|| async {
            let (player, _recorder) = player();
            let settings = test_settings();
            let lease = player.claim(&settings).expect("a player");
            let transmission = cq();
            let plan = cw_core::plan_transmission(&transmission, &settings);
            let asked = lease.ask_to_send();
            let outcome = lease
                .send_planned(asked, &plan, &transmission, &settings, None, |_| {
                    lease.stop_sending()
                })
                .await;
            assert_eq!(outcome, Err(AudioError::Cancelled));
        });
    }

    /// A send is the one asked for from when it was asked: a stop before
    /// it starts ends it, and so does a newer send asked for while it waits
    /// out a retry, which it never starts over.
    #[test]
    fn a_send_asked_for_is_ended_by_a_stop_or_a_newer_one() {
        run(|| async {
            let (player, recorder) = player();
            let settings = test_settings();
            let lease = player.claim(&settings).expect("a player");
            let transmission = cq();
            let plan = cw_core::plan_transmission(&transmission, &settings);
            let send =
                |asked| lease.send_planned(asked, &plan, &transmission, &settings, None, |_| {});

            let asked = lease.ask_to_send();
            lease.stop_sending();
            assert_eq!(send(asked).await, Err(AudioError::Cancelled));
            assert!(recorder.texts().is_empty(), "a stopped send started");

            // The first attempt breaks at once, and the newer send is asked
            // for halfway through the gap before its retry.
            recorder.fail_next(1);
            let first = send(lease.ask_to_send());
            let newer = async {
                sleep_ms(POLL_MS / 2).await;
                send(lease.ask_to_send()).await
            };
            let (first, newer) = tokio::join!(first, newer);
            assert_eq!(first, Err(AudioError::Cancelled));
            assert!(newer.is_ok(), "{newer:?}");
            assert_eq!(recorder.texts().len(), 2, "the first started again");
            assert!(
                player.0.backend.borrow().is_some(),
                "the newer lost its player"
            );

            // A lease that has ended cannot end the holder's send.
            let holder = player.claim(&settings).expect("a player");
            let asked = holder.ask_to_send();
            let _ = lease.ask_to_send();
            let sent = holder
                .send_planned(asked, &plan, &transmission, &settings, None, |_| {})
                .await;
            assert!(sent.is_ok(), "{sent:?}");
        });
    }

    /// A player that failed every attempt is thrown away, so the next claim
    /// opens a fresh one.
    #[test]
    fn a_player_that_failed_every_attempt_is_opened_afresh() {
        run(|| async {
            let (player, recorder) = player();
            let lease = player.claim(&test_settings()).expect("a player");
            recorder.set(Behaviour::FailMidSend);
            assert_eq!(
                lease.send(&cq(), &test_settings(), |_| {}).await,
                Err(AudioError::Stalled)
            );
            assert!(player.0.backend.borrow().is_none());
            recorder.set(Behaviour::Play);
            let built = recorder.players_built.get();
            let lease = player.claim(&test_settings()).expect("a player");
            assert_eq!(recorder.players_built.get(), built + 1);
            assert!(lease.send(&cq(), &test_settings(), |_| {}).await.is_ok());
        });
    }

    /// Waking opens a player that is gone and resumes it, for the lease that
    /// holds the audio only.
    #[test]
    fn waking_opens_a_missing_player_for_the_lease_that_holds_it() {
        run(|| async {
            let (player, recorder) = player();
            let lease = player.claim(&test_settings()).expect("a player");
            player.0.backend.borrow_mut().take();
            lease.wake();
            assert_eq!(recorder.players_built.get(), 2);
            player.0.backend.borrow_mut().take();
            player.release();
            lease.wake();
            assert_eq!(recorder.players_built.get(), 2, "an ended lease woke it");
        });
    }

    #[test]
    fn every_failure_says_what_happened() {
        assert_eq!(
            AudioError::Device("No audio output device found".into()).to_string(),
            "No audio output device found"
        );
        assert_eq!(AudioError::Stalled.to_string(), "Audio playback stalled.");
    }
}
