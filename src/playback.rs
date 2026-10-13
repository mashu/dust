//! Everything the app plays outside a training session: the band preview,
//! the letter player, the group stream and the envelope samples.

use cw_core::{
    CALLSIGN_TIER_MAX, CharSamplingState, GroupSource, Sampler, SplitMix64, TrainingSettings,
    Transmission, compute_after_group_gap_ms, compute_group_gap_for_wpm, generate_callsign,
    generate_training_group, resolve_station,
};
use dioxus::core::spawn_forever;
use dioxus::prelude::*;

use crate::player::{AudioError, Lease};
use crate::state::AppCtx;
use crate::time::seed_rng;

/// How many sent groups the stream keeps on screen for a glance-back.
const STREAM_HEARD_KEEP: usize = 12;

/// A group the stream has sent, and how fast it was sent.
#[derive(Clone, Debug, PartialEq)]
pub struct HeardGroup {
    pub text: String,
    /// The speed of the characters themselves.
    pub char_wpm: f64,
    /// The overall speed, spacing included; below `char_wpm` under Farnsworth.
    pub effective_wpm: f64,
}

impl HeardGroup {
    /// "24 WPM", or "24 WPM · 18 eff." when the gaps are stretched.
    pub fn speed_label(&self) -> String {
        let char_wpm = self.char_wpm.round();
        let effective_wpm = self.effective_wpm.round();
        if effective_wpm < char_wpm {
            format!("{char_wpm:.0} WPM · {effective_wpm:.0} eff.")
        } else {
            format!("{char_wpm:.0} WPM")
        }
    }
}

/// The pause between letters on the Listen screen.
const LETTER_GAP_MS: u32 = 420;

/// What the band preview calls, and the pause between calls.
const BAND_PREVIEW_CALL: &str = "CQ";
const BAND_PREVIEW_GAP_MS: u32 = 280;

/// What the app is playing when no session is running.
///
/// These are the four things that can be true of the audio outside training,
/// and exactly one of them is: the band preview, the letter player and an
/// envelope sample all want the same hardware, so a second one starting means
/// the first has stopped.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Preview {
    /// Nothing is playing, or a session has the audio.
    #[default]
    Idle,
    /// The band-conditions preview, looping until stopped.
    Band,
    /// The Listen screen playing a character or the whole pool.
    Letters,
    /// Groups looping with no answers and no score — copy while you do something else.
    Stream,
    /// One short envelope sample, named so the chip that sent it can show it.
    Sample(String),
}

impl Preview {
    pub fn is_band(&self) -> bool {
        matches!(self, Self::Band)
    }

    pub fn is_letters(&self) -> bool {
        matches!(self, Self::Letters)
    }

    pub fn is_stream(&self) -> bool {
        matches!(self, Self::Stream)
    }

    /// The sample now sounding, for the chip that is waiting to light up.
    pub fn sample(&self) -> Option<String> {
        match self {
            Self::Sample(text) => Some(text.clone()),
            _ => None,
        }
    }
}

/// Play `kind`: take the audio for it, run `play` under the lease, and once
/// that ends — finished, failed or taken over — leave nothing behind.
///
/// Every preview starts and ends here, so none of them can forget a step:
/// the preview shown is the one playing, a failure is said once, and an end
/// that is still this preview's own gives the audio back, receiver and all.
fn start_preview<F>(ctx: AppCtx, kind: Preview, play: impl FnOnce(Lease, TrainingSettings) -> F)
where
    F: Future<Output = Result<(), AudioError>> + 'static,
{
    // A session in progress keeps the audio.
    if ctx.session_running() {
        return;
    }
    let AppCtx {
        mut preview,
        toasts,
        settings,
        ..
    } = ctx;
    let settings = settings.peek().clone().clamp();
    let lease = match ctx.app().player.claim(&settings) {
        Ok(lease) => lease,
        Err(err) => {
            // The claim ended whatever was playing, whether or not it could
            // open the player for this.
            preview.set(Preview::Idle);
            toasts.show(err.to_string());
            return;
        }
    };
    preview.set(kind);
    let playing = play(lease.clone(), settings);
    // The app's, not the screen's: the lease decides when it ends.
    spawn_forever(async move {
        if let Err(err) = playing.await
            && err != AudioError::Cancelled
        {
            toasts.show(err.to_string());
        }
        if lease.release() {
            preview.set(Preview::Idle);
        }
    });
}

/// The Listen screen: `chars`, one at a time.
pub fn start_letters(ctx: AppCtx, chars: String) {
    start_preview(ctx, Preview::Letters, |lease, settings| {
        play_chars(lease, settings, chars, LETTER_GAP_MS)
    });
}

/// The Listen screen's stream of groups, nothing scored.
pub fn start_stream(ctx: AppCtx) {
    let AppCtx {
        settings,
        sessions,
        mut stream_heard,
        ..
    } = ctx;
    start_preview(ctx, Preview::Stream, |lease, first| {
        stream_heard.set(Vec::new());
        // Leaning on the same weak characters a session would.
        let sampler = Sampler::new(&sessions.peek(), &first, seed_rng());
        loop_stream(lease, settings, sampler, stream_heard)
    });
}

/// An envelope test chip: one short sample with the current keying settings.
pub fn start_sample(ctx: AppCtx, text: String) {
    start_preview(ctx, Preview::Sample(text.clone()), |lease, settings| {
        play_sample(lease, settings, text)
    });
}

/// The band-conditions preview, calling until it is stopped.
pub fn start_band_preview(ctx: AppCtx) {
    start_preview(ctx, Preview::Band, |lease, _| {
        loop_band_preview(lease, ctx.settings, BAND_PREVIEW_CALL, BAND_PREVIEW_GAP_MS)
    });
}

/// One operator for a whole preview, drawn afresh each time one starts.
fn new_station(settings: &TrainingSettings) -> (cw_core::StationVoice, SplitMix64) {
    let mut rng = SplitMix64::new(seed_rng());
    (resolve_station(settings, &mut rng), rng)
}

/// Play `chars` one at a time, `gap_ms` apart.
async fn play_chars(
    lease: Lease,
    settings: TrainingSettings,
    chars: String,
    gap_ms: u32,
) -> Result<(), AudioError> {
    // One operator for the whole run: a different pitch per letter would make
    // the screen harder to learn from, not more realistic.
    let (voice, _) = new_station(&settings);
    for (i, ch) in chars.chars().enumerate() {
        if i > 0 {
            lease.sleep(gap_ms).await?;
        }
        lease
            .send(&Transmission::alone(ch.to_string(), voice), &settings)
            .await?;
    }
    Ok(())
}

/// Send one short sample once — the keying-envelope test chips.
async fn play_sample(
    lease: Lease,
    settings: TrainingSettings,
    text: String,
) -> Result<(), AudioError> {
    let (voice, _) = new_station(&settings);
    lease
        .send(&Transmission::alone(text, voice), &settings)
        .await
        .map(|_| ())
}

/// Call `text` over and over, `gap_ms` apart, under the settings as they are
/// at each call. Ends only when the lease does, or the audio fails.
async fn loop_band_preview(
    lease: Lease,
    settings: Signal<TrainingSettings>,
    text: &'static str,
    gap_ms: u32,
) -> Result<(), AudioError> {
    // One station, kept, so moving a band slider changes the band and not
    // the signal you are judging it against. Whoever calls across it is drawn
    // afresh each time, from the settings as they are now, so the pile-up is
    // heard here just as a session would send it.
    let (voice, mut rng) = new_station(&settings.peek().clone().clamp());
    loop {
        let settings_now = settings.peek().clone().clamp();
        // A CQ is answered by callsigns.
        let call = Transmission::calling(&settings_now, text, voice, &mut rng, |rng| {
            generate_callsign(CALLSIGN_TIER_MAX, rng)
        });
        lease.send(&call, &settings_now).await?;
        lease.sleep(gap_ms).await?;
    }
}

/// Send groups from the current alphabet until the lease ends, showing each
/// in `heard` once it has played.
///
/// This is not a training session. Its sampler is one of its own — built
/// from the history as a session's is, so it leans on the same weak
/// characters — and nothing it sends is answered, so nothing is learned and
/// nothing is stored. Each group is sent the way a session sends it, though:
/// by a station of its own, drawn from the speed, tone and strength ranges
/// in the settings as they are when it starts.
async fn loop_stream(
    lease: Lease,
    settings: Signal<TrainingSettings>,
    mut sampler: Sampler,
    mut heard: Signal<Vec<HeardGroup>>,
) -> Result<(), AudioError> {
    let mut rng = SplitMix64::new(seed_rng());
    loop {
        let settings_now = settings.peek().clone().clamp();
        let voice = resolve_station(&settings_now, &mut rng);
        let (group, repeats) = sampler.next_group(&settings_now);
        // Across it, whoever else the settings have calling, sending groups
        // of their own as a session's callers do: from the same curriculum,
        // off a blank slate, so they never teach the sampler anything.
        let call = Transmission::calling(&settings_now, group.clone(), voice, &mut rng, |rng| {
            generate_training_group(&settings_now, &mut CharSamplingState::default(), rng)
        });
        // Sent as often as a session would send it, a word space apart.
        let mut sent = lease.send(&call, &settings_now).await?;
        for _ in 1..repeats.max(1) {
            lease
                .sleep(compute_group_gap_for_wpm(
                    sent.char_wpm,
                    sent.effective_wpm,
                    settings_now.playback.extra_word_space_multiplier,
                ))
                .await?;
            sent = lease.send(&call, &settings_now).await?;
        }
        heard.with_mut(|list| {
            list.push(HeardGroup {
                text: group,
                char_wpm: sent.char_wpm,
                effective_wpm: sent.effective_wpm,
            });
            let extra = list.len().saturating_sub(STREAM_HEARD_KEEP);
            list.drain(..extra);
        });
        let gap = compute_after_group_gap_ms(
            sent.char_wpm,
            sent.effective_wpm,
            settings_now.playback.extra_word_space_multiplier,
            settings_now.playback.group_pause_sec,
        );
        lease.sleep(gap).await?;
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use super::*;
    use crate::audio::fake::Behaviour;
    use crate::testing::{Harness, run, test_settings};
    use cw_core::SessionResult;

    /// What a spawned preview came back with, once it has.
    type Outcome = Rc<RefCell<Option<Result<(), AudioError>>>>;

    fn spawn_into<F>(h: &Harness, play: F) -> Outcome
    where
        F: Future<Output = Result<(), AudioError>> + 'static,
    {
        let outcome: Outcome = Rc::default();
        let sink = outcome.clone();
        h.in_app(|| {
            spawn(async move {
                let result = play.await;
                *sink.borrow_mut() = Some(result);
            })
        });
        outcome
    }

    fn sampler(h: &Harness, history: &[SessionResult]) -> Sampler {
        // A fixed seed, so what the stream draws is the same every run.
        Sampler::new(history, &h.ctx.settings.peek(), 7)
    }

    fn claim(h: &Harness) -> Lease {
        h.app
            .player
            .claim(&test_settings())
            .expect("the fake always opens")
    }

    /// A wait is one timer, not a run of short ones.
    ///
    /// Counting down in poll-sized steps woke sixty times a second for as
    /// long as the wait ran — an answer window superseded early kept doing
    /// so to its end — and charged each step what it asked for rather than
    /// what it took, so every gap ran long.
    #[test]
    fn a_long_wait_wakes_once() {
        run(|| async {
            let h = Harness::new();
            let lease = claim(&h);
            let polls = Rc::new(Cell::new(0u32));
            let wait = Polls {
                inner: Box::pin(lease.sleep(10_000)),
                polls: polls.clone(),
            };
            let started = tokio::time::Instant::now();
            assert_eq!(wait.await, Ok(()), "nothing cancelled it");
            assert_eq!(started.elapsed().as_millis(), 10_000);
            assert!(polls.get() <= 2, "the wait woke {} times", polls.get());
        });
    }

    /// Counts how often the future inside is polled.
    struct Polls<F> {
        inner: std::pin::Pin<Box<F>>,
        polls: Rc<Cell<u32>>,
    }

    impl<F: Future> Future for Polls<F> {
        type Output = F::Output;

        fn poll(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<F::Output> {
            self.polls.set(self.polls.get() + 1);
            self.inner.as_mut().poll(cx)
        }
    }

    #[test]
    fn listening_plays_one_character_at_a_time() {
        run(|| async {
            let mut h = Harness::new();
            let lease = claim(&h);
            let outcome = spawn_into(&h, play_chars(lease, test_settings(), "KM".into(), 200));
            h.pump();
            assert!(h.run_until(10_000, |_| outcome.borrow().is_some()).await);
            assert_eq!(*outcome.borrow(), Some(Ok(())));
            assert_eq!(h.texts(), vec!["K".to_string(), "M".to_string()]);
        });
    }

    #[test]
    fn listening_stops_when_the_user_leaves() {
        run(|| async {
            let mut h = Harness::new();
            let lease = claim(&h);
            let outcome = spawn_into(&h, play_chars(lease, test_settings(), "KMU".into(), 400));
            h.pump();
            assert!(h.run_until(5_000, |h| h.texts().len() == 1).await);
            h.app.player.release();
            h.advance(5_000).await;
            assert_eq!(h.texts().len(), 1, "nothing is played after leaving");
            assert_eq!(*outcome.borrow(), Some(Err(AudioError::Cancelled)));
        });
    }

    #[test]
    fn listening_gives_up_on_the_first_character_the_audio_fails() {
        run(|| async {
            let mut h = Harness::new();
            let lease = claim(&h);
            h.set_behaviour(Behaviour::RefuseToStart);
            let outcome = spawn_into(&h, play_chars(lease, test_settings(), "KM".into(), 200));
            h.pump();
            assert!(h.run_until(20_000, |_| outcome.borrow().is_some()).await);
            assert_eq!(
                *outcome.borrow(),
                Some(Err(AudioError::Device(
                    "Audio stream: device is gone".into()
                )))
            );
            // It gave up on the first character rather than grinding through both.
            assert!(h.texts().iter().all(|text| text == "K"));
        });
    }

    #[test]
    fn a_test_sample_plays_once() {
        run(|| async {
            let mut h = Harness::new();
            let lease = claim(&h);
            let outcome = spawn_into(&h, play_sample(lease, test_settings(), "CQ".into()));
            h.pump();
            assert!(h.run_until(10_000, |_| outcome.borrow().is_some()).await);
            assert_eq!(*outcome.borrow(), Some(Ok(())));
            assert_eq!(h.texts(), vec!["CQ".to_string()]);
        });
    }

    #[test]
    fn the_band_preview_loops_until_it_is_stopped() {
        run(|| async {
            let mut h = Harness::new();
            let lease = claim(&h);
            let outcome = spawn_into(&h, loop_band_preview(lease, h.ctx.settings, "CQ", 100));
            h.pump();
            assert!(h.run_until(20_000, |h| h.texts().len() >= 3).await);
            assert!(h.texts().iter().all(|text| text == "CQ"));
            h.app.player.release();
            let played = h.texts().len();
            h.advance(5_000).await;
            assert_eq!(h.texts().len(), played);
            assert_eq!(*outcome.borrow(), Some(Err(AudioError::Cancelled)));
        });
    }

    /// "Stations calling" is heard in the preview too: each CQ comes with a
    /// fresh pile-up of as many others as the settings ask for, none of them
    /// sending what the wanted station sends.
    ///
    /// Through a wide filter, so there is room for every one of them: the
    /// default 300 Hz receiver often has room for only one caller beside a
    /// station a little off the middle of the side-tone range, and drops the
    /// rest, as a narrow filter does.
    #[test]
    fn the_band_preview_brings_the_pile_up_the_settings_ask_for() {
        run(|| async {
            let mut settings = test_settings();
            settings.band.stations_min = 3;
            settings.band.stations_max = 3;
            settings.band.filter_width_hz = cw_core::FILTER_WIDTH_MAX;
            let mut h = Harness::with_settings(settings);
            let lease = claim(&h);
            let _outcome = spawn_into(&h, loop_band_preview(lease, h.ctx.settings, "CQ", 100));
            h.pump();
            assert!(h.run_until(20_000, |h| h.texts().len() >= 3).await);
            h.app.player.release();
            let sent = h.recorder.sent();
            assert!(sent.iter().all(|t| t.text == "CQ" && t.others.len() <= 2));
            assert!(sent.iter().any(|t| t.others.len() == 2), "no pile-up");
            assert!(
                sent.iter()
                    .flat_map(|t| t.others.iter())
                    .all(|other| other.text != "CQ" && !other.text.is_empty())
            );
            // The same station calls every time: only the pile-up is new.
            assert!(sent.windows(2).all(|w| w[0].voice == w[1].voice));
        });
    }

    #[test]
    fn the_band_preview_gives_up_when_the_audio_fails() {
        run(|| async {
            let mut h = Harness::new();
            let lease = claim(&h);
            h.set_behaviour(Behaviour::RefuseToStart);
            let outcome = spawn_into(&h, loop_band_preview(lease, h.ctx.settings, "CQ", 100));
            h.pump();
            assert!(h.run_until(20_000, |_| outcome.borrow().is_some()).await);
            assert!(matches!(
                *outcome.borrow(),
                Some(Err(AudioError::Device(_)))
            ));
            let played = h.texts().len();
            h.advance(5_000).await;
            assert_eq!(h.texts().len(), played, "the loop stopped");
        });
    }

    #[test]
    fn a_send_that_is_taken_over_mid_flight_is_cancelled() {
        run(|| async {
            let mut h = Harness::new();
            let lease = claim(&h);
            let outcome = spawn_into(&h, play_sample(lease, test_settings(), "CQ".into()));
            h.pump();
            h.advance(100).await;
            assert!(outcome.borrow().is_none(), "still sending");
            // Another screen claims the audio.
            let _other = claim(&h);
            assert!(h.run_until(5_000, |_| outcome.borrow().is_some()).await);
            assert_eq!(*outcome.borrow(), Some(Err(AudioError::Cancelled)));
        });
    }

    #[test]
    fn a_group_stream_keeps_sending_until_it_is_stopped() {
        run(|| async {
            let mut h = Harness::new();
            let lease = claim(&h);
            let heard = h.in_app(|| Signal::new(Vec::<HeardGroup>::new()));
            let _outcome = spawn_into(
                &h,
                loop_stream(lease, h.ctx.settings, sampler(&h, &[]), heard),
            );
            h.pump();
            assert!(h.run_until(20_000, |h| h.texts().len() >= 3).await);
            assert!(
                h.texts().iter().all(|text| text.len() == 2),
                "each send is one training group"
            );
            assert!(
                heard.peek().len() >= 2,
                "finished groups show up after they play"
            );
            h.app.player.release();
            let played = h.texts().len();
            h.advance(5_000).await;
            assert_eq!(h.texts().len(), played, "the stream stopped");
        });
    }

    /// Each group comes from a station of its own, drawn from the ranges in
    /// the settings, as in a session. The stream used to draw one station as
    /// it started and send everything at its speed and pitch.
    #[test]
    fn a_group_stream_draws_each_group_from_the_ranges_in_the_settings() {
        run(|| async {
            let mut settings = test_settings();
            settings.playback.char_wpm_min = 15.0;
            settings.playback.char_wpm_max = 40.0;
            settings.playback.link_char_to_effective = true;
            settings.band.side_tone_min = 400.0;
            settings.band.side_tone_max = 900.0;
            let mut h = Harness::with_settings(settings);
            let lease = claim(&h);
            let heard = h.in_app(|| Signal::new(Vec::<HeardGroup>::new()));
            let _outcome = spawn_into(
                &h,
                loop_stream(lease, h.ctx.settings, sampler(&h, &[]), heard),
            );
            h.pump();
            assert!(h.run_until(120_000, |h| h.texts().len() >= 6).await);
            h.app.player.release();
            let voices = h.recorder.voices();
            assert!(
                voices
                    .iter()
                    .all(|v| (15.0..=40.0).contains(&v.char_wpm)
                        && (400.0..=900.0).contains(&v.tone_hz)),
                "{voices:?}"
            );
            assert!(
                voices.windows(2).any(|w| w[0].char_wpm != w[1].char_wpm),
                "every group was sent at one speed: {voices:?}"
            );
            assert!(
                voices.windows(2).any(|w| w[0].tone_hz != w[1].tone_hz),
                "every group was sent at one pitch: {voices:?}"
            );
            // What is shown is the speed each one was sent at.
            let shown = heard.peek().clone();
            assert!(!shown.is_empty());
            for (group, voice) in shown.iter().zip(&voices) {
                assert_eq!(group.char_wpm, voice.char_wpm);
            }
        });
    }

    /// Repeats are sent as a session sends them: the same group, from the
    /// same station, the set number of times, and shown once.
    #[test]
    fn a_group_stream_sends_each_group_as_often_as_the_settings_say() {
        run(|| async {
            let mut settings = test_settings();
            settings.playback.group_repeat_min = 3;
            settings.playback.group_repeat_max = 3;
            let mut h = Harness::with_settings(settings);
            let lease = claim(&h);
            let heard = h.in_app(|| Signal::new(Vec::<HeardGroup>::new()));
            let _outcome = spawn_into(
                &h,
                loop_stream(lease, h.ctx.settings, sampler(&h, &[]), heard),
            );
            h.pump();
            assert!(h.run_until(120_000, |h| h.texts().len() >= 6).await);
            h.app.player.release();
            let texts = h.texts();
            let voices = h.recorder.voices();
            for (group_texts, group_voices) in texts.chunks(3).zip(voices.chunks(3)).take(2) {
                assert!(group_texts.windows(2).all(|w| w[0] == w[1]), "{texts:?}");
                assert!(group_voices.windows(2).all(|w| w[0] == w[1]), "{voices:?}");
            }
            assert!(heard.peek().len() <= texts.len().div_ceil(3));
        });
    }

    #[test]
    fn a_heard_group_says_how_fast_it_was_sent() {
        let group = |char_wpm, effective_wpm| HeardGroup {
            text: "KM".into(),
            char_wpm,
            effective_wpm,
        };
        assert_eq!(group(24.2, 24.2).speed_label(), "24 WPM");
        assert_eq!(group(25.0, 18.4).speed_label(), "25 WPM · 18 eff.");
    }

    /// The stream leans on what the history says is hard, as a session does.
    /// It used to draw from a blank slate before the first session of a
    /// launch, and from whatever that session had learned after it.
    #[test]
    fn a_group_stream_leans_on_the_characters_the_history_misses() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.level = 2;
            settings.curriculum.num_groups = 40;
            settings.curriculum.min_group_size = 1;
            settings.curriculum.max_group_size = 1;
            let mut h = Harness::with_settings(settings);
            // Every K copied, every M and U missed, over and over.
            let sessions: Vec<SessionResult> = (0..20)
                .map(|_| {
                    crate::testing::stored_session(
                        "2026-09-01",
                        &[("K", "K"), ("M", "E"), ("U", "E"), ("K", "K")],
                    )
                })
                .collect();
            assert!(sessions[0].usable_for_sampling(&h.ctx.settings.peek()));
            let lease = claim(&h);
            let heard = h.in_app(|| Signal::new(Vec::<HeardGroup>::new()));
            let _outcome = spawn_into(
                &h,
                loop_stream(lease, h.ctx.settings, sampler(&h, &sessions), heard),
            );
            h.pump();
            assert!(h.run_until(240_000, |h| h.texts().len() >= 60).await);
            h.app.player.release();
            let ks = h.texts().iter().filter(|text| *text == "K").count();
            // Drawn evenly, a third of them would be; from a blank slate this
            // seed draws eighteen.
            assert!(ks < 14, "{ks} of 60 were the character always copied");
        });
    }

    #[test]
    fn a_group_stream_gives_up_when_the_audio_fails() {
        run(|| async {
            let mut h = Harness::new();
            let lease = claim(&h);
            h.set_behaviour(Behaviour::RefuseToStart);
            let heard = h.in_app(|| Signal::new(Vec::<HeardGroup>::new()));
            let outcome = spawn_into(
                &h,
                loop_stream(lease, h.ctx.settings, sampler(&h, &[]), heard),
            );
            h.pump();
            assert!(h.run_until(20_000, |_| outcome.borrow().is_some()).await);
            let played = h.texts().len();
            h.advance(5_000).await;
            assert_eq!(h.texts().len(), played, "the stream stopped");
            assert!(heard.peek().is_empty());
        });
    }
}
