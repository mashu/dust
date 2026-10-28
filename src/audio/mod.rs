//! Audio backends and the waiting that goes with them.
//!
//! Every backend behind [`MorseBackend`] plays the same [`cw_core::PlaybackPlan`]
//! and reports the same [`WaitFlags`], so the session runtime never has to know
//! which one it is talking to.

mod epoch;
#[cfg(any(feature = "native-audio", feature = "web"))]
mod mixer;
#[cfg(feature = "native-audio")]
mod native;
#[cfg(any(feature = "native-audio", feature = "web"))]
mod shape;
#[cfg(feature = "silent-audio")]
mod silent;
#[cfg(any(feature = "web", all(test, feature = "native-audio")))]
mod stream;
mod tone;
mod wait;
#[cfg(feature = "web")]
mod web;

pub use tone::ToneRequest;
pub use wait::{PlaybackOutcome, PlaybackSignal, PlaybackWait, SendTiming, WaitFlags};

use std::rc::Rc;

use cw_core::{MonoMs, PlannedTransmission, TrainingSettings, Transmission, plan_transmission};

use crate::time::mono_ms;

/// True when the build has no audio output and only simulates the timing of a
/// session.
pub const AUDIO_IS_SILENT: bool = cfg!(feature = "silent-audio");

/// How long one meter frame covers: short enough to show a dit at the
/// fastest speed as a dit, long enough that the scope is a few hundred
/// points.
pub const METER_FRAME_SEC: f64 = 0.01;

/// What the receiver did over one short stretch, for the level scope: the
/// loudest of what reached its AGC, the band and the Morse together through
/// the filter; the gain the AGC rode at, at its lowest; and the loudest of
/// what it played, at the volume, before the limiter.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MeterFrame {
    pub input: f32,
    pub gain: f32,
    pub output: f32,
}

/// What the session runtime needs from a player. Object-safe on purpose: the
/// runtime holds one boxed backend and tests put their own in its place.
pub trait MorseBackend {
    /// Browsers only start audio inside a user gesture; everywhere else this
    /// does nothing.
    fn resume_from_gesture(&mut self) {}

    /// Bring the background layers in line with the settings, and the
    /// volume everything but the paddle's sidetone is played at; the
    /// sidetone comes at the level it is heard at.
    ///
    /// Never fails. A device that will not open is not the band's failure:
    /// the band is in place for when it does, and the next send reports the
    /// device — which is what sends the session round its rebuild-and-retry.
    fn apply_band(&mut self, settings: &TrainingSettings);

    /// Start one send, through the receiver `settings` describe, and hand
    /// back what to watch it by.
    ///
    /// `transmission` is the send as the session asked for it: the station
    /// being copied, and whoever else is calling across it. `plan` is how it
    /// plays — every station's tones against one clock — and is all a player
    /// needs; it is planned once, by [`start_send`], for every backend.
    ///
    /// `heard_in`, for a send that is to start at a given time, is how long
    /// from now its sound should reach the ear: a contest's continuation,
    /// which takes over from the send playing where that one gives way
    /// ([`cw_core::Turn::starts_at`]). The player lets the send playing run
    /// on to there and starts the new one then, against the clock the old
    /// one plays on; once that moment is past, or with nothing playing, as
    /// near it as it can. Its signal says when it is really heard. None
    /// starts it as soon as it can.
    fn start(
        &mut self,
        transmission: &Transmission,
        plan: &PlannedTransmission,
        settings: &TrainingSettings,
        heard_in: Option<f64>,
    ) -> Result<Rc<dyn PlaybackSignal>, String>;

    /// Silence the Morse, leaving the background alone.
    fn stop(&mut self);

    /// Silence everything, background included.
    fn shutdown(&mut self);

    /// Let the output sleep, silenced already: nothing sounds until the next
    /// band, send or sidetone, which wakes it. A stream left running with
    /// nothing to play wakes the audio thread hundreds of times a second,
    /// and keeps the sound device, and the sound server's graph, from ever
    /// resting. The browser's player sleeps by its own clock.
    fn sleep(&mut self) {}

    /// Gate a live sidetone for paddle practice: on or off, its edge sounded
    /// a constant lead after the instant the keyer meant it for, so each
    /// element and space is as long as it was keyed. Kept as a running
    /// stream so a squeeze is that lead late, not a stream start.
    fn set_live_tone(&mut self, tone: ToneRequest) {
        let _ = tone;
    }

    /// The receiver's meter frames since the last were taken, oldest first,
    /// appended to `into`. None from a player with nothing to meter.
    fn take_meter(&mut self, into: &mut Vec<MeterFrame>) {
        let _ = into;
    }
}

/// Send `transmission` on `player`, and hand back something to wait on. The
/// caller owns the stations, so every repeat of a group arrives from the same
/// operators.
///
/// The band is brought in line first, so a send whose settings moved it is
/// heard over the band they describe. The send is planned here, once, and the
/// wait reads its length and speeds off that plan — whichever backend plays
/// it, and the test double too.
pub fn start_send(
    player: &mut dyn MorseBackend,
    transmission: &Transmission,
    settings: &TrainingSettings,
) -> Result<PlaybackWait, String> {
    let plan = plan_transmission(transmission, settings);
    start_planned(player, &plan, transmission, settings, None)
}

/// Send a send planned already on `player`, and hand back something to wait
/// on: a contest's turn, every reply in it whole and your message stretched
/// to the last of them, which [`plan_transmission`] would cut. `transcript`
/// is what the turn says, for a player that keeps it; `plan` is all a player
/// plays. `starts_at`, for a continuation, is when its sound is to reach
/// the ear ([`MorseBackend::start`]).
///
/// The band is brought in line first, as for any send, and the wait reads
/// its length off the plan.
pub fn start_planned(
    player: &mut dyn MorseBackend,
    plan: &PlannedTransmission,
    transcript: &Transmission,
    settings: &TrainingSettings,
    starts_at: Option<MonoMs>,
) -> Result<PlaybackWait, String> {
    player.apply_band(settings);
    // Read once the band is in line, a device reopened or a band rebuilt,
    // and before the player schedules: the moment it reckons its lead from.
    // Earlier, and that work would be counted as the sound's delay; later,
    // and the scheduling would be taken off it.
    let started = MonoMs(mono_ms());
    let heard_in = starts_at.map(|at| (at.0 as f64 - started.0 as f64) / 1_000.0);
    let signal = player.start(transcript, plan, settings, heard_in)?;
    Ok(PlaybackWait::new(&plan.wanted, signal, started))
}

/// The player this build ships with.
pub fn default_backend() -> Result<Box<dyn MorseBackend>, String> {
    #[cfg(feature = "native-audio")]
    {
        Ok(Box::new(native::MorsePlayer::new()?))
    }
    #[cfg(feature = "web")]
    {
        Ok(Box::new(web::MorsePlayer::new()?))
    }
    #[cfg(feature = "silent-audio")]
    {
        Ok(Box::new(silent::MorsePlayer::new()?))
    }
}

/// A player that records what it was asked to do and plays nothing. Session
/// tests drive the whole runtime through it.
#[cfg(test)]
pub mod fake {
    use super::epoch::{Epoch, Ticket};
    use super::*;
    use cw_core::band::Qsb;
    use std::cell::{Cell, RefCell};

    #[derive(Clone, Debug, PartialEq)]
    pub enum Call {
        New,
        Band(cw_core::band::BandKey),
        Start(String),
        Stop,
        Shutdown,
        /// The paddle sidetone gated on or off.
        LiveTone(bool),
    }

    /// How the next send should behave.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Behaviour {
        /// Plays for its full length and finishes.
        Play,
        /// `start` refuses.
        RefuseToStart,
        /// Starts, then reports a broken stream.
        FailMidSend,
        /// Starts and never gets anywhere: a stalled audio clock.
        Stall,
    }

    /// One send as the player was handed it: what it plays, the fading it
    /// plays with, what it says, and when it was to start.
    #[derive(Clone, Debug)]
    pub struct Started {
        /// What it plays, whether its wanted send went past the receiver,
        /// as your message in a contest does, and when the receiver was to
        /// be muted.
        pub plan: PlannedTransmission,
        pub qsb: Option<Qsb>,
        pub transcript: Transmission,
        /// For a send that was to start at a given time, how long after it
        /// was started its sound was to reach the ear.
        pub heard_in: Option<f64>,
        /// When it was handed over, on the app's clock: with no wait, when
        /// its first sample goes out.
        pub at: MonoMs,
    }

    #[derive(Default)]
    pub struct Recorder {
        pub calls: RefCell<Vec<Call>>,
        pub behaviour: Cell<Option<Behaviour>>,
        pub players_built: Cell<u32>,
        pub build_error: Cell<bool>,
        /// The page is out of sight, the way a hidden browser tab is: the
        /// audio clock parks, and nothing charges the stall budget.
        pub page_hidden: Cell<bool>,
        /// Every send, in order, so a test can hear whether a repeated group
        /// came from the same operator and who else was calling, and what a
        /// contest's turn played and said.
        pub starts: RefCell<Vec<Started>>,
        /// Whether the receiver background is running. The real player keeps
        /// it on its own stream, outliving any one send, so a test that only
        /// watches sends cannot tell when it has been left playing.
        pub band_running: Cell<bool>,
        /// Sends that break before the good ones start, whatever `behaviour`
        /// says. One per send, so a recovery can be tested exactly.
        pub failures_left: Cell<u32>,
        /// How long after it starts each send says it reaches the ear: none,
        /// unless a test asks for some.
        pub latency_sec: Cell<f64>,
        /// Meter frames for the level scope to take, as a test puts them.
        pub meter: RefCell<Vec<MeterFrame>>,
        /// The output is asleep, as the native player's stream sleeps: until
        /// a band, a send or the sidetone wakes it.
        pub asleep: Cell<bool>,
        /// How many times it was put to sleep.
        pub sleeps: Cell<u32>,
    }

    impl Recorder {
        pub fn new() -> Rc<Self> {
            Rc::new(Self {
                behaviour: Cell::new(Some(Behaviour::Play)),
                ..Self::default()
            })
        }

        pub fn behaviour(&self) -> Behaviour {
            self.behaviour.get().unwrap_or(Behaviour::Play)
        }

        pub fn set(&self, behaviour: Behaviour) {
            self.behaviour.set(Some(behaviour));
        }

        /// Break the next `count` sends, then play normally.
        pub fn fail_next(&self, count: u32) {
            self.failures_left.set(count);
        }

        /// Send the page to the background, or bring it back.
        pub fn set_page_hidden(&self, hidden: bool) {
            self.page_hidden.set(hidden);
        }

        pub fn calls(&self) -> Vec<Call> {
            self.calls.borrow().clone()
        }

        pub fn texts(&self) -> Vec<String> {
            self.calls
                .borrow()
                .iter()
                .filter_map(|call| match call {
                    Call::Start(text) => Some(text.clone()),
                    _ => None,
                })
                .collect()
        }

        pub fn clear(&self) {
            self.calls.borrow_mut().clear();
            self.starts.borrow_mut().clear();
        }

        pub fn sent(&self) -> Vec<Transmission> {
            self.starts
                .borrow()
                .iter()
                .map(|start| start.transcript.clone())
                .collect()
        }

        pub fn voices(&self) -> Vec<cw_core::StationVoice> {
            self.starts
                .borrow()
                .iter()
                .map(|start| start.transcript.voice)
                .collect()
        }

        pub fn starts(&self) -> Vec<Started> {
            self.starts.borrow().clone()
        }

        pub fn factory(self: &Rc<Self>) -> Rc<dyn Fn() -> Result<Box<dyn MorseBackend>, String>> {
            let recorder = Rc::clone(self);
            Rc::new(move || {
                if recorder.build_error.get() {
                    return Err("No audio output device found".to_string());
                }
                recorder.players_built.set(recorder.players_built.get() + 1);
                recorder.calls.borrow_mut().push(Call::New);
                Ok(Box::new(FakePlayer {
                    recorder: Rc::clone(&recorder),
                    epoch: Epoch::default(),
                }))
            })
        }
    }

    pub struct FakePlayer {
        recorder: Rc<Recorder>,
        epoch: Epoch,
    }

    struct FakeSignal {
        recorder: Rc<Recorder>,
        ticket: Ticket,
        duration_ms: u32,
        played: Cell<u32>,
        latency_sec: f64,
        /// How long it waits before it starts, a send asked to start at a
        /// given time, in the ticks of its clock.
        wait_ms: u32,
    }

    impl PlaybackSignal for FakeSignal {
        fn timing(&self) -> SendTiming {
            SendTiming {
                lead_sec: f64::from(self.wait_ms) / 1_000.0,
                latency_sec: self.latency_sec,
            }
        }

        fn poll(&self) -> WaitFlags {
            if self.ticket.is_cancelled() {
                return WaitFlags {
                    cancelled: true,
                    ..Default::default()
                };
            }
            if self.recorder.failures_left.get() > 0 {
                self.recorder
                    .failures_left
                    .set(self.recorder.failures_left.get() - 1);
                return WaitFlags {
                    failed: true,
                    ..Default::default()
                };
            }
            if self.recorder.page_hidden.get() {
                // A backgrounded page: the clock stops where it is, and the
                // scheduled tone is still waiting to be heard.
                return WaitFlags {
                    suspended: true,
                    played_ms: Some(self.played.get()),
                    ..Default::default()
                };
            }
            match self.recorder.behaviour() {
                Behaviour::FailMidSend => WaitFlags {
                    failed: true,
                    ..Default::default()
                },
                Behaviour::Stall => WaitFlags {
                    played_ms: Some(0),
                    ..Default::default()
                },
                _ => {
                    // Each poll advances the "audio clock" by one tick, so a
                    // send takes as long as its plan says it does, from when
                    // it was to start, and is heard its latency after, as a
                    // real player counts it.
                    let latency_ms = (self.latency_sec * 1_000.0).round() as u32;
                    let held = self.wait_ms.saturating_add(latency_ms);
                    let played = self.played.get().saturating_add(crate::time::POLL_MS).min(
                        self.duration_ms
                            .saturating_add(wait::PLAYBACK_TAIL_MS)
                            .saturating_add(held),
                    );
                    self.played.set(played);
                    WaitFlags {
                        played_ms: Some(played.saturating_sub(held)),
                        ..Default::default()
                    }
                }
            }
        }
    }

    impl MorseBackend for FakePlayer {
        fn apply_band(&mut self, settings: &TrainingSettings) {
            self.recorder.asleep.set(false);
            self.recorder
                .calls
                .borrow_mut()
                .push(Call::Band(cw_core::band::BandKey::of(&settings.band)));
            self.recorder
                .band_running
                .set(settings.band.has_background());
        }

        fn start(
            &mut self,
            transmission: &Transmission,
            plan: &PlannedTransmission,
            settings: &TrainingSettings,
            heard_in: Option<f64>,
        ) -> Result<Rc<dyn PlaybackSignal>, String> {
            self.recorder.asleep.set(false);
            self.recorder
                .calls
                .borrow_mut()
                .push(Call::Start(transmission.text.clone()));
            self.recorder.starts.borrow_mut().push(Started {
                plan: plan.clone(),
                qsb: Qsb::of(&settings.band),
                transcript: transmission.clone(),
                heard_in,
                at: MonoMs(mono_ms()),
            });
            if self.recorder.behaviour() == Behaviour::RefuseToStart {
                return Err("Audio stream: device is gone".to_string());
            }
            // The wanted station sets the length; the others are laid over it.
            let duration_ms = (plan.wanted.duration_sec * 1000.0).ceil().max(0.0) as u32;
            // Started at its time, as a real player starts a continuation,
            // or at once once that is past.
            let latency_sec = self.recorder.latency_sec.get();
            let wait_sec = heard_in.map_or(0.0, |heard_in| (heard_in - latency_sec).max(0.0));
            Ok(Rc::new(FakeSignal {
                recorder: Rc::clone(&self.recorder),
                ticket: self.epoch.arm(),
                duration_ms,
                played: Cell::new(0),
                latency_sec,
                wait_ms: (wait_sec * 1_000.0).round() as u32,
            }))
        }

        fn stop(&mut self) {
            self.recorder.calls.borrow_mut().push(Call::Stop);
            self.epoch.bump();
        }

        fn shutdown(&mut self) {
            self.recorder.calls.borrow_mut().push(Call::Shutdown);
            self.recorder.band_running.set(false);
            self.epoch.bump();
        }

        fn sleep(&mut self) {
            self.recorder.asleep.set(true);
            self.recorder.sleeps.set(self.recorder.sleeps.get() + 1);
        }

        fn set_live_tone(&mut self, tone: ToneRequest) {
            self.recorder.asleep.set(false);
            self.recorder
                .calls
                .borrow_mut()
                .push(Call::LiveTone(tone.on));
        }

        fn take_meter(&mut self, into: &mut Vec<MeterFrame>) {
            into.append(&mut self.recorder.meter.borrow_mut());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{Call, Recorder};
    use super::*;

    /// A send brings its band with it, on every backend alike: a send whose
    /// settings moved the band is heard over the band they describe, and the
    /// wait is as long as the plan says, at the speeds it resolved to.
    #[test]
    fn a_send_is_heard_over_its_own_band_for_as_long_as_its_plan() {
        let recorder = Recorder::new();
        let mut player = (recorder.factory())().expect("a player");
        let mut settings = TrainingSettings::default();
        settings.band.noise_enabled = true;
        settings.band.noise_level = 0.4;
        let voice = cw_core::resolve_station(&settings, &mut cw_core::SplitMix64::new(3));
        let transmission = Transmission::alone("K", voice);

        let wait = start_send(player.as_mut(), &transmission, &settings).expect("a send");
        assert_eq!(
            recorder.calls(),
            vec![
                Call::New,
                Call::Band(cw_core::band::BandKey::of(&settings.band)),
                Call::Start("K".into()),
            ]
        );
        assert!(recorder.band_running.get());

        let plan = plan_transmission(&transmission, &settings).wanted;
        assert_eq!(wait.duration_sec, plan.duration_sec);
        let start = wait.start();
        assert_eq!(start.char_wpm, plan.resolved_char_wpm);
        assert_eq!(start.effective_wpm, plan.resolved_effective_wpm);
    }
}
