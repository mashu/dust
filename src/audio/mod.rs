//! Audio backends and the waiting that goes with them.
//!
//! Every backend behind [`MorseBackend`] plays the same [`cw_core::PlaybackPlan`]
//! and reports the same [`WaitFlags`], so the session runtime never has to know
//! which one it is talking to.

#[cfg(feature = "native-audio")]
mod native;
#[cfg(any(feature = "native-audio", test))]
pub mod render;
#[cfg(feature = "silent-audio")]
mod silent;
mod wait;
#[cfg(feature = "web")]
mod web;

#[cfg_attr(not(test), allow(unused_imports))]
pub use wait::{PlaybackOutcome, PlaybackSignal, PlaybackWait, WaitFlags};

use cw_core::{StationVoice, TrainingSettings};

/// True when the build has no audio output and only simulates the timing of a
/// session.
pub const AUDIO_IS_SILENT: bool = cfg!(feature = "silent-audio");

/// What the session runtime needs from a player. Object-safe on purpose: the
/// runtime holds one boxed backend and tests put their own in its place.
pub trait MorseBackend {
    /// Browsers only start audio inside a user gesture; everywhere else this
    /// does nothing.
    fn resume_from_gesture(&mut self) {}

    /// Bring the background layers in line with the settings.
    fn apply_band(&mut self, settings: &TrainingSettings) -> Result<(), String>;

    /// Schedule one send and hand back something to wait on.
    /// Send `text` as one station. The caller owns the voice so every repeat
    /// of a group arrives from the same operator.
    fn start_text(
        &mut self,
        text: &str,
        settings: &TrainingSettings,
        voice: &StationVoice,
    ) -> Result<PlaybackWait, String>;

    /// Silence the Morse, leaving the background alone.
    fn stop(&mut self);

    /// Silence everything, background included.
    fn shutdown(&mut self);

    /// The promise for a resumed browser audio context, if one is pending.
    #[cfg(feature = "web")]
    fn take_resume_promise(&self) -> Option<js_sys::Promise> {
        None
    }
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
    #[cfg(not(any(feature = "native-audio", feature = "web", feature = "silent-audio")))]
    {
        Err("This build has no audio backend.".to_string())
    }
}

pub fn focus_group_input(index: usize) {
    let js = format!(
        r#"(() => {{
            const cardId = "group-card-{index}";
            const inputId = "group-input-{index}";
            let scrolled = false;
            const apply = () => {{
                const card = document.getElementById(cardId);
                if (card && !scrolled) {{
                    card.scrollIntoView({{ behavior: "smooth", block: "center", inline: "nearest" }});
                    scrolled = true;
                }}
                const el = document.getElementById(inputId);
                if (!el || el.disabled) {{
                    return;
                }}
                if (document.activeElement !== el) {{
                    el.focus({{ preventScroll: true }});
                }}
            }};
            apply();
            requestAnimationFrame(() => {{
                apply();
                setTimeout(apply, 50);
                setTimeout(apply, 180);
            }});
        }})()"#
    );
    let _ = dioxus::document::eval(&js);
}

/// A player that records what it was asked to do and plays nothing. Session
/// tests drive the whole runtime through it.
#[cfg(test)]
pub mod fake {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    #[derive(Clone, Debug, PartialEq)]
    pub enum Call {
        New,
        Band(String),
        Start(String),
        Stop,
        Shutdown,
    }

    /// How the next send should behave.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Behaviour {
        /// Plays for its full length and finishes.
        Play,
        /// `start_text` refuses.
        RefuseToStart,
        /// Starts, then reports a broken stream.
        FailMidSend,
        /// Starts and never gets anywhere: a stalled audio clock.
        Stall,
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
        /// The station behind every send, in order, so a test can hear whether
        /// a repeated group came from the same operator.
        pub voices: RefCell<Vec<StationVoice>>,
        /// Sends that break before the good ones start, whatever `behaviour`
        /// says. One per send, so a recovery can be tested exactly.
        pub failures_left: Cell<u32>,
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
            self.voices.borrow_mut().clear();
        }

        pub fn voices(&self) -> Vec<StationVoice> {
            self.voices.borrow().clone()
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
                    epoch: Rc::new(Cell::new(0)),
                }))
            })
        }
    }

    pub struct FakePlayer {
        recorder: Rc<Recorder>,
        epoch: Rc<Cell<u64>>,
    }

    struct FakeSignal {
        recorder: Rc<Recorder>,
        epoch: Rc<Cell<u64>>,
        mine: u64,
        duration_ms: u32,
        played: Cell<u32>,
    }

    impl PlaybackSignal for FakeSignal {
        fn poll(&self) -> WaitFlags {
            if self.epoch.get() != self.mine {
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
                    // send takes as long as its plan says it does.
                    let played = self
                        .played
                        .get()
                        .saturating_add(crate::time::POLL_MS)
                        .min(self.duration_ms.saturating_add(wait::PLAYBACK_TAIL_MS));
                    self.played.set(played);
                    WaitFlags {
                        played_ms: Some(played),
                        ..Default::default()
                    }
                }
            }
        }
    }

    impl MorseBackend for FakePlayer {
        fn apply_band(&mut self, settings: &TrainingSettings) -> Result<(), String> {
            self.recorder
                .calls
                .borrow_mut()
                .push(Call::Band(settings.band_signature()));
            Ok(())
        }

        fn start_text(
            &mut self,
            text: &str,
            settings: &TrainingSettings,
            voice: &StationVoice,
        ) -> Result<PlaybackWait, String> {
            self.recorder
                .calls
                .borrow_mut()
                .push(Call::Start(text.to_string()));
            self.recorder.voices.borrow_mut().push(*voice);
            if self.recorder.behaviour() == Behaviour::RefuseToStart {
                return Err("Audio stream: device is gone".to_string());
            }
            let plan = cw_core::plan_morse_playback_for(text, settings, voice);
            self.epoch.set(self.epoch.get() + 1);
            let duration_ms = (plan.duration_sec * 1000.0).ceil().max(0.0) as u32;
            Ok(PlaybackWait::new(
                plan.duration_sec,
                plan.resolved_char_wpm,
                plan.resolved_effective_wpm,
                Rc::new(FakeSignal {
                    recorder: Rc::clone(&self.recorder),
                    epoch: Rc::clone(&self.epoch),
                    mine: self.epoch.get(),
                    duration_ms,
                    played: Cell::new(0),
                }),
            ))
        }

        fn stop(&mut self) {
            self.recorder.calls.borrow_mut().push(Call::Stop);
            self.epoch.set(self.epoch.get() + 1);
        }

        fn shutdown(&mut self) {
            self.recorder.calls.borrow_mut().push(Call::Shutdown);
            self.epoch.set(self.epoch.get() + 1);
        }
    }
}
