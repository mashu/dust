//! Everything the app plays outside a training session: the band preview,
//! the letter player, the group stream, the envelope samples, and Hear a
//! QSO.

use std::cell::Cell;
use std::rc::Rc;

use cw_core::band::agc_decay_sec;
use cw_core::{
    AgcDecay, CALLSIGN_TIER_MAX, CharSamplingState, ContestKind, GroupSource, HeardTurn, MonoMs,
    Sampler, Sender, SplitMix64, StationVoice, TrainingSettings, Transmission, VOLUME_MAX,
    VOLUME_MIN, compute_after_group_gap_ms, compute_group_gap_for_wpm, generate_callsign,
    generate_training_group, hear_qsos, plan_turn, resolve_station,
};
use dioxus::core::spawn_forever;
use dioxus::prelude::*;

use crate::player::{AudioError, Lease, SendNews};
use crate::state::AppCtx;
use crate::time::{mono_ms, seed_rng};

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

/// How many QSOs Hear a QSO plays: a few, enough to hear one follow
/// another.
const HEAR_QSOS: u32 = 3;

/// What Hear a QSO has played so far, for a screen to show as a dialogue.
#[derive(Clone, Debug, PartialEq)]
pub struct Dialogue {
    /// The contest the QSOs are of.
    pub kind: ContestKind,
    /// Your station as they send it, where some of it stands in for yours.
    pub stand_in: Option<String>,
    /// How many QSOs it plays.
    pub qsos: u32,
    /// Each line heard whole so far, in the order they ended.
    pub lines: Vec<DialogueLine>,
}

impl Dialogue {
    /// The QSO it has reached: the one its newest line is part of.
    pub fn qso(&self) -> u32 {
        self.lines.last().map_or(1, |line| line.qso)
    }
}

/// One line of a dialogue: your message, or what callers sent at once, as
/// a pile-up's callers do.
#[derive(Clone, Debug, PartialEq)]
pub struct DialogueLine {
    /// Who sent it: you, the first of the callers, or QRM.
    pub who: Sender,
    /// What was sent, a send each, in the order they ended.
    pub sends: Vec<String>,
    /// Which of the QSOs it is part of, counted from 1.
    pub qso: u32,
}

/// What the band preview calls.
pub(crate) const BAND_PREVIEW_CALL: &str = "CQ";

/// The pause between the band preview's calls: long enough for the AGC at
/// Fast to let the band back after the loudest of them. Its hang and five of
/// its release's time constants bring a 26 dB duck within a fifth of a
/// decibel, so the hiss is heard coming back and the scope's lamp goes out a
/// quarter of a second or so before the next call, which then starts on a
/// gain of its own. At Slow the band is still held down at the end of it,
/// which is what Slow is.
pub(crate) fn band_preview_gap_ms() -> u32 {
    let (hang, release) = agc_decay_sec(AgcDecay::Fast);
    ((hang + 5.0 * release) * 1_000.0).round() as u32
}

/// The strengths the band preview calls at, in turn: loud, medium and weak,
/// the full strength and a station 10 and 20 dB down, so the AGC can be
/// heard evening them out and its threshold heard deciding which it does.
pub const BAND_PREVIEW_STRENGTHS: [f64; 3] = [VOLUME_MAX, 0.316_227_766_016_838, VOLUME_MIN];

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
    /// A few QSOs of the chosen contest, worked by an operator who copies
    /// every character, for a learner to hear before working one.
    Qsos,
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

    pub fn is_qsos(&self) -> bool {
        matches!(self, Self::Qsos)
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
        mut dialogue,
        toasts,
        settings,
        ..
    } = ctx;
    // What Hear a QSO played stays up only until something else plays.
    if kind != Preview::Qsos {
        dialogue.set(None);
    }
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
        loop_band_preview(
            lease,
            ctx.settings,
            BAND_PREVIEW_CALL,
            band_preview_gap_ms(),
        )
    });
}

/// Hear a QSO: a few QSOs of the contest the settings choose, worked by
/// the perfect operator and played as a run plays its turns, each send
/// shown once it has been heard whole.
///
/// This is not a run. The QSOs are worked through before the first is
/// played, by an operator that hears their turns only, as it would hear a
/// run's; the sampler their calls come from is one of its own, built from
/// the history as a run's is, and let go of once they are worked. Nothing
/// is stored, and no stat hears of them.
pub fn start_qsos(ctx: AppCtx) {
    let AppCtx {
        sessions,
        mut dialogue,
        ..
    } = ctx;
    start_preview(ctx, Preview::Qsos, |lease, settings| {
        let sampler = Sampler::new(&sessions.peek(), &settings, seed_rng());
        let heard = hear_qsos(seed_rng(), &settings, sampler, HEAR_QSOS);
        let kind = settings.contest.contest_kind;
        dialogue.set(Some(Dialogue {
            kind,
            stand_in: heard.stand_in,
            qsos: HEAR_QSOS,
            lines: Vec::new(),
        }));
        let signoff = kind.rules().signoff;
        let sends = sends_of(&heard.turns, signoff);
        // Played as a run plays: each caller's fading is in its tones, so
        // the send itself has none.
        let mut sound = settings;
        sound.band.qsb_enabled = false;
        play_qsos(lease, sound, heard.turns, sends, dialogue)
    });
}

/// Play `turns`, each after the quiet before it, through the same player
/// path as a run's: your message dry, around the receiver, and the callers
/// through it. Each of a turn's `sends` shows in `dialogue` once it has
/// been heard whole, and every one of them once the turn is over.
async fn play_qsos(
    lease: Lease,
    sound: TrainingSettings,
    turns: Vec<HeardTurn>,
    sends: Vec<Vec<HeardSend>>,
    dialogue: Signal<Option<Dialogue>>,
) -> Result<(), AudioError> {
    // Which start of a send may show its lines: a send tried again starts
    // its lines over, and one over leaves none for its timers to show.
    let showing = Rc::new(Cell::new(0u64));
    let mut before = 0;
    for (HeardTurn { quiet_ms, turn }, sends) in turns.into_iter().zip(sends) {
        if quiet_ms > 0 {
            lease
                .sleep(u32::try_from(quiet_ms).unwrap_or(u32::MAX))
                .await?;
        }
        let Some(transcript) = turn.transcript() else {
            continue;
        };
        let plan = plan_turn(&turn, &sound);
        let asked = lease.ask_to_send();
        let started = {
            let (lease, showing, sends) = (lease.clone(), showing.clone(), sends.clone());
            move |news: SendNews<'_>| {
                let SendNews::Started(start) = news else {
                    return;
                };
                let heard_after_sec = start.heard_at().since(MonoMs(mono_ms())) as f64 / 1_000.0;
                showing.set(showing.get() + 1);
                let shows = (showing.clone(), showing.get());
                spawn_forever(show_sends(
                    lease.clone(),
                    sends.clone(),
                    before,
                    heard_after_sec,
                    shows,
                    dialogue,
                ));
            }
        };
        // The perfect operator keys only once the air is quiet, so no turn
        // it hears is a continuation: each starts after the quiet before it.
        lease
            .send_planned(asked, &plan, &transcript, &sound, None, started)
            .await?;
        showing.set(showing.get() + 1);
        show(dialogue, before, &sends);
        before += lines_of(&sends).len();
    }
    Ok(())
}

/// One send of QSOs heard: when, into its turn, it starts and ends, who
/// sent what, and which of the QSOs it is part of.
#[derive(Clone, Debug)]
struct HeardSend {
    start_sec: f64,
    end_sec: f64,
    who: Sender,
    text: String,
    qso: u32,
}

/// The sends of each of `turns`, in the order they end. A QSO closes with
/// your TU, and with its station's `signoff` where the contest has one;
/// the next caller to send begins the next QSO. QRM, a station on your
/// frequency that is not calling you, is heard as it comes, but begins no
/// QSO.
fn sends_of(turns: &[HeardTurn], signoff: Option<&str>) -> Vec<Vec<HeardSend>> {
    let mut qso = 1;
    let mut closed = false;
    turns
        .iter()
        .map(|heard| {
            let mut segments: Vec<_> = heard.turn.segments.iter().collect();
            segments.sort_by(|a, b| a.end_sec.total_cmp(&b.end_sec));
            segments
                .into_iter()
                .map(|segment| {
                    let text = &segment.text;
                    if segment.who == Sender::You {
                        closed |= text.split_whitespace().any(|word| word == "TU");
                    } else if segment.who.is_caller() && closed && Some(text.as_str()) != signoff {
                        qso += 1;
                        closed = false;
                    }
                    HeardSend {
                        start_sec: segment.start_sec,
                        end_sec: segment.end_sec,
                        who: segment.who,
                        text: text.clone(),
                        qso,
                    }
                })
                .collect()
        })
        .collect()
}

/// `sends` as lines: each its own, but callers sending at once for the
/// same QSO, one starting before another has ended, share one. QRM is a
/// line of its own.
fn lines_of(sends: &[HeardSend]) -> Vec<DialogueLine> {
    let mut lines: Vec<DialogueLine> = Vec::new();
    let mut ended_sec = 0.0;
    for send in sends {
        let at_once = send.who.is_caller()
            && send.start_sec < ended_sec
            && lines
                .last()
                .is_some_and(|line| line.who.is_caller() && line.qso == send.qso);
        match lines.last_mut() {
            Some(line) if at_once => line.sends.push(send.text.clone()),
            _ => lines.push(DialogueLine {
                who: send.who,
                sends: vec![send.text.clone()],
                qso: send.qso,
            }),
        }
        ended_sec = send.end_sec;
    }
    lines
}

/// Show `sends`, those of a turn after the `before` lines of the turns
/// before it, each as it is heard whole, `heard_after_sec` after now, for
/// as long as `shows` is the start of the send that may show them.
async fn show_sends(
    lease: Lease,
    sends: Vec<HeardSend>,
    before: usize,
    heard_after_sec: f64,
    shows: (Rc<Cell<u64>>, u64),
    dialogue: Signal<Option<Dialogue>>,
) {
    let (showing, mine) = shows;
    let mut waited_ms = 0;
    for heard in 1..=sends.len() {
        let due_ms = ((heard_after_sec + sends[heard - 1].end_sec) * 1_000.0).ceil() as u32;
        if due_ms > waited_ms {
            if lease.sleep(due_ms - waited_ms).await.is_err() {
                return;
            }
            waited_ms = due_ms;
        }
        if showing.get() != mine {
            return;
        }
        show(dialogue, before, &sends[..heard]);
    }
}

/// The lines of `sends` shown after the `before` lines already up.
fn show(mut dialogue: Signal<Option<Dialogue>>, before: usize, sends: &[HeardSend]) {
    dialogue.with_mut(|dialogue| {
        if let Some(dialogue) = dialogue {
            dialogue.lines.truncate(before);
            dialogue.lines.extend(lines_of(sends));
        }
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
            .send(
                &Transmission::alone(ch.to_string(), voice),
                &settings,
                |_| {},
            )
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
        .send(&Transmission::alone(text, voice), &settings, |_| {})
        .await
        .map(|_| ())
}

/// Call `text` over and over, `gap_ms` apart, loud, medium and weak in turn
/// ([`BAND_PREVIEW_STRENGTHS`]), under the settings as they are at each
/// call. Ends only when the lease does, or the audio fails.
async fn loop_band_preview(
    lease: Lease,
    settings: Signal<TrainingSettings>,
    text: &'static str,
    gap_ms: u32,
) -> Result<(), AudioError> {
    // One station, kept, so moving a band slider changes the band and not
    // the signal you are judging it against: the same pitch and speed every
    // time, only its strength stepping round. Whoever calls across it is
    // drawn afresh each time, from the settings as they are now, so the
    // pile-up is heard here just as a session would send it.
    let (station, mut rng) = new_station(&settings.peek().clone().clamp());
    let mut turn = 0;
    loop {
        let strength = BAND_PREVIEW_STRENGTHS[turn % BAND_PREVIEW_STRENGTHS.len()];
        turn += 1;
        let settings_now = settings.peek().clone().clamp();
        let voice = StationVoice {
            volume: strength,
            ..station
        };
        // A CQ is answered by callsigns.
        let call = Transmission::calling(&settings_now, text, voice, &mut rng, |rng| {
            generate_callsign(CALLSIGN_TIER_MAX, rng)
        });
        lease.send(&call, &settings_now, |_| {}).await?;
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
        let mut voice = resolve_station(&settings_now, &mut rng);
        let (group, repeats) = sampler.next_group(&settings_now);
        // With adaptive speed on, the stream is sent where a session would
        // send it.
        if let Some((char_wpm, effective_wpm)) = sampler.next_speed(&settings_now) {
            voice.char_wpm = char_wpm;
            voice.effective_wpm = effective_wpm;
        }
        // Across it, whoever else the settings have calling, sending groups
        // of their own as a session's callers do: from the same curriculum,
        // off a blank slate, so they never teach the sampler anything.
        let call = Transmission::calling(&settings_now, group.clone(), voice, &mut rng, |rng| {
            generate_training_group(&settings_now, &mut CharSamplingState::default(), rng)
        });
        // Sent as often as a session would send it, a word space apart.
        let mut sent = lease.send(&call, &settings_now, |_| {}).await?;
        for _ in 1..repeats.max(1) {
            lease
                .sleep(compute_group_gap_for_wpm(
                    sent.char_wpm,
                    sent.effective_wpm,
                    settings_now.playback.extra_word_space_multiplier,
                ))
                .await?;
            sent = lease.send(&call, &settings_now, |_| {}).await?;
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

    /// QRM is heard as it comes, a line of its own that no caller shares,
    /// and begins no QSO after your TU: the next caller does.
    #[test]
    fn qrm_is_a_line_of_its_own_and_begins_no_qso() {
        use cw_core::{HeardTurn, Segment, Sender, SplitMix64, Turn, resolve_station};
        let voice = resolve_station(&test_settings(), &mut SplitMix64::new(1));
        let segment = |who, text: &str, start_sec, end_sec| Segment {
            who,
            text: text.to_string(),
            voice,
            start_sec,
            end_sec,
            fade: None,
            skip_sec: 0.0,
        };
        let turn = |segments| HeardTurn {
            quiet_ms: 0,
            turn: Turn {
                id: 1,
                at_run_ms: 0,
                starts_at: None,
                segments,
            },
        };
        let turns = [
            turn(vec![
                segment(Sender::You, "K5XYZ 5NN TT1", 0.0, 2.0),
                segment(Sender::Qrm, "QRL?", 0.0, 1.0),
                segment(Sender::Caller(0), "TU 5NN 214", 2.3, 4.0),
            ]),
            turn(vec![
                segment(Sender::You, "TU", 0.0, 0.5),
                segment(Sender::Qrm, "CQ CQ TEST K5ABC K5ABC TEST", 1.0, 6.0),
                segment(Sender::Caller(1), "W7AB", 0.8, 1.5),
            ]),
        ];
        let sends = sends_of(&turns, None);
        let said: Vec<Vec<&str>> = sends
            .iter()
            .map(|sends| sends.iter().map(|send| send.text.as_str()).collect())
            .collect();
        assert_eq!(
            said,
            [
                vec!["QRL?", "K5XYZ 5NN TT1", "TU 5NN 214"],
                vec!["TU", "W7AB", "CQ CQ TEST K5ABC K5ABC TEST"]
            ]
        );
        let lines: Vec<(Sender, u32)> = sends
            .iter()
            .flat_map(|sends| lines_of(sends))
            .map(|line| (line.who, line.qso))
            .collect();
        assert_eq!(
            lines,
            [
                (Sender::Qrm, 1),
                (Sender::You, 1),
                (Sender::Caller(0), 1),
                (Sender::You, 1),
                (Sender::Caller(1), 2),
                (Sender::Qrm, 2),
            ],
            "W7AB begins the next, the QRM none"
        );
        // A caller sending while QRM does shares no line with it.
        let at_once = [
            HeardSend {
                start_sec: 0.0,
                end_sec: 2.0,
                who: Sender::Caller(0),
                text: "K5XYZ".to_string(),
                qso: 1,
            },
            HeardSend {
                start_sec: 0.5,
                end_sec: 2.5,
                who: Sender::Qrm,
                text: "QRL?".to_string(),
                qso: 1,
            },
        ];
        assert_eq!(lines_of(&at_once).len(), 2);
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
            // The same station calls every time, loud, medium and weak in
            // turn: only its strength and the pile-up are new.
            let same = |a: &StationVoice, b: &StationVoice| {
                StationVoice {
                    volume: b.volume,
                    ..*a
                } == *b
            };
            assert!(sent.windows(2).all(|w| same(&w[0].voice, &w[1].voice)));
            let strengths: Vec<f64> = sent.iter().map(|t| t.voice.volume).collect();
            assert_eq!(strengths[..3], BAND_PREVIEW_STRENGTHS);
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
