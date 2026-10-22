//! On-frequency QRM, under rough habits: now and then a station that is not
//! calling you starts up on your frequency, as MorseRunner's QRM stations
//! do (`QrmStn.pas`, `Contest.pas:783`). It sends `QRL?`, a long CQ, or
//! your call and `QSY QSY`, fast, then a long CQ or a few more, seconds
//! apart, and goes. It never hears you and never answers: it is no caller,
//! is never counted as calling, and is no part of a QSO.
//!
//! The players hold one send at a time, so QRM is heard inside a turn: its
//! messages go into the turns on the air as they come due, and one that
//! came due while the frequency was quiet goes out as the next turn starts.
//! Its draws come from a generator of its own, so a run under easier habits
//! draws as it always has.

use super::caller::FADE_OFFSET_MAX_SEC;
use super::place::place_qrm;
use super::rules::{ContestRules, Side};
use super::turn::{Fade, Segment, Sender, sending_sec};
use crate::band::Qsb;
use crate::callsign::generate_call;
use crate::rng::{Rng, SplitMix64};
use crate::session::whole_ms;
use crate::settings::{LevelAxis, TrainingSettings};
use crate::timing::{StationVoice, resolve_station};

/// How long a run goes, on average, between one station starting up on
/// your frequency and the next: MorseRunner starts one in 0.0002 of its
/// blocks of 512 samples at 11,025 Hz (`Contest.pas:783`), whether or not
/// one is sending, so one in about 232 seconds.
const QRM_EVERY_MS: f64 = 512.0 / 11_025.0 / 0.0002 * 1_000.0;

/// How many messages a station sends before it goes, fewest and most
/// (`QrmStn.pas`: `1 + Random(5)`).
const MESSAGES: (usize, usize) = (1, 5);

/// How fast it sends, slowest and fastest, in WPM, its spacing as its
/// characters (`QrmStn.pas`: `30 + Random(20)`).
const WPM: (usize, usize) = (30, 49);

/// How long it waits after a message before the next, in milliseconds:
/// four seconds, give or take two (`QrmStn.pas`: `RndGaussLim(4 s, 2 s)`).
const GAP_MS: f64 = 4_000.0;
const GAP_SPREAD_MS: f64 = 2_000.0;

/// What a station sends first, by a draw from 0 to 6 (`QrmStn.pas`:
/// `Random(7)`): `QRL?` for 0, `QRL? QRL?` up to 2, a long CQ up to 5, and
/// your call with `QSY QSY` for 6.
const QRL_ONCE: usize = 0;
const QRL_TWICE: usize = 2;
const LONG_CQ: usize = 5;
const FIRST_DRAW_MAX: usize = 6;

/// The QRM of a run: when the next station starts up, when the frequency
/// is next free of it, and the station sending now, if any.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Qrm {
    rules: &'static ContestRules,
    side: Side,
    /// Your call, for a station to tell to QSY.
    my_call: String,
    /// The band's fading, which each station follows from a point of its
    /// own; none with the Band card's QSB off.
    qsb: Option<Qsb>,
    rng: SplitMix64,
    /// When the next station starts up, in run time: each start comes a
    /// wait after the one before, whether or not a station still sends,
    /// so they come as often as MorseRunner's.
    next_ms: u64,
    /// When the next message may go, in run time: a gap after the last one
    /// ends. A station's next message waits for it, and so does one that
    /// starts up while another still sends, rather than send over it.
    free_ms: u64,
    station: Option<QrmStation>,
}

/// A station sending QRM on your frequency.
#[derive(Clone, Debug, PartialEq)]
struct QrmStation {
    call: String,
    voice: StationVoice,
    fade_from_sec: f64,
    /// Messages still to send.
    left: usize,
    /// Its first message is still to go.
    first: bool,
}

impl Qrm {
    /// The QRM of a run of `rules` on `side`, as `my_call`, on the band
    /// `settings` set, its draws from `rng`.
    pub(super) fn new(
        settings: &TrainingSettings,
        rules: &'static ContestRules,
        side: Side,
        my_call: &str,
        mut rng: SplitMix64,
    ) -> Self {
        let next_ms = wait_ms(&mut rng);
        Self {
            rules,
            side,
            my_call: my_call.to_string(),
            qsb: Qsb::of(&settings.band),
            rng,
            next_ms,
            free_ms: 0,
            station: None,
        }
    }

    /// The messages it sends in a turn keyed at `run_ms` that lasts
    /// `length_sec` as planned so far, timed as `settings` play them: each
    /// that comes due before the turn is over, as it comes due, or as the
    /// turn starts for one due while nothing was on the air. A message is
    /// never cut: one that starts late in the turn runs on past it.
    pub(super) fn sends(
        &mut self,
        run_ms: u64,
        length_sec: f64,
        settings: &TrainingSettings,
    ) -> Vec<Segment> {
        let end_ms = run_ms + whole_ms(length_sec);
        let mut sends = Vec::new();
        loop {
            let mut station = match self.station.take() {
                Some(station) => station,
                None if self.next_ms < end_ms => self.start_up(run_ms, settings),
                None => break,
            };
            if self.free_ms >= end_ms {
                self.station = Some(station);
                break;
            }
            let start_ms = self.free_ms.max(run_ms);
            let text = station.message(self.rules, &self.my_call, &mut self.rng);
            let start_sec = (start_ms - run_ms) as f64 / 1_000.0;
            let end_sec = start_sec + sending_sec(&text, settings, &station.voice);
            sends.push(Segment {
                who: Sender::Qrm,
                text,
                voice: station.voice,
                start_sec,
                end_sec,
                fade: Fade::along(self.qsb, station.fade_from_sec, run_ms, start_sec),
            });
            self.free_ms = run_ms + whole_ms(end_sec) + gap_ms(&mut self.rng);
            station.left -= 1;
            if station.left > 0 {
                self.station = Some(station);
            }
        }
        sends
    }

    /// The rest of a message carried into a turn keyed at `run_ms`, from
    /// the character sounding at your key, ends `end_sec` into it: later,
    /// it may be, than its message was planned to end, so nothing more of
    /// QRM goes until a gap after it, and none sends over it.
    pub(super) fn carried(&mut self, run_ms: u64, end_sec: f64) {
        let free_ms = run_ms + whole_ms(end_sec) + gap_ms(&mut self.rng);
        self.free_ms = self.free_ms.max(free_ms);
    }

    /// The next station starts up for a turn keyed at `run_ms`, its first
    /// message due as it does, or once the last one is done: any station of
    /// the contest, at the callsign tier, fast, near your pitch. The start
    /// after it is drawn from this one's.
    fn start_up(&mut self, run_ms: u64, settings: &TrainingSettings) -> QrmStation {
        let rng = &mut self.rng;
        self.free_ms = self.free_ms.max(self.next_ms);
        self.next_ms += wait_ms(rng);
        // Of the stations that started up while the frequency was quiet
        // before the turn, only the last is still there: the others came
        // and went with nothing on the air, so a long silence brings one
        // station, not a queue of them.
        while self.free_ms <= run_ms && self.next_ms < run_ms {
            self.next_ms += wait_ms(rng);
        }
        let tier = settings.level(LevelAxis::Callsign);
        let call = generate_call(tier, self.rules.callers(self.side), rng).text;
        let placed = place_qrm(settings, rng);
        let mut voice = resolve_station(settings, rng);
        let wpm = rng.usize_in(WPM.0, WPM.1) as f64;
        voice.char_wpm = wpm;
        voice.effective_wpm = wpm;
        voice.tone_hz = placed.tone_hz;
        voice.volume = placed.volume;
        QrmStation {
            call,
            voice,
            fade_from_sec: rng.pick_in_range(0.0, FADE_OFFSET_MAX_SEC),
            left: rng.usize_in(MESSAGES.0, MESSAGES.1),
            first: true,
        }
    }
}

#[cfg(test)]
impl Qrm {
    /// The next station starts up at `at_ms` of run time instead, for a
    /// test to hear one when it wants.
    pub(super) fn start_at(&mut self, at_ms: u64) {
        self.next_ms = at_ms;
    }
}

impl QrmStation {
    /// Its next message to `my_call`: first `QRL?`, a long CQ or your call
    /// and `QSY QSY`, and a long CQ after.
    fn message(&mut self, rules: &ContestRules, my_call: &str, rng: &mut impl Rng) -> String {
        let first = std::mem::take(&mut self.first);
        let long_cq = rules.long_cq_text(&self.call);
        if !first {
            return long_cq;
        }
        match rng.usize_in(0, FIRST_DRAW_MAX) {
            QRL_ONCE => "QRL?".to_string(),
            kind if kind <= QRL_TWICE => "QRL? QRL?".to_string(),
            kind if kind <= LONG_CQ || my_call.is_empty() => long_cq,
            _ => format!("{my_call} QSY QSY"),
        }
    }
}

/// How long from one station starting up to the next, in milliseconds: an
/// exponential wait of mean [`QRM_EVERY_MS`], as a chance in each moment.
fn wait_ms(rng: &mut impl Rng) -> u64 {
    let uniform = (1.0 - rng.f64()).clamp(f64::EPSILON, 1.0);
    (-uniform.ln() * QRM_EVERY_MS).round() as u64
}

/// How long a station waits after a message before the next.
fn gap_ms(rng: &mut impl Rng) -> u64 {
    rng.normal_within(GAP_MS, GAP_SPREAD_MS).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contest::ContestKind;

    fn settings() -> TrainingSettings {
        TrainingSettings::default().clamp()
    }

    /// The QRM of a WPX run as DL1ABC, from `seed`.
    fn wpx(seed: u64) -> Qrm {
        let rules = ContestKind::Wpx.rules();
        Qrm::new(
            &settings(),
            rules,
            Side::Dx,
            "DL1ABC",
            SplitMix64::new(seed),
        )
    }

    /// Every message sent in turns of `turn_sec`, `quiet_sec` apart, over
    /// `run_minutes` of run time, each with the run time its turn was
    /// keyed at.
    fn heard(seed: u64, turn_sec: f64, quiet_sec: f64, run_minutes: u64) -> Vec<(u64, Segment)> {
        let mut qrm = wpx(seed);
        let mut sends = Vec::new();
        let mut run_ms = 0;
        while run_ms < run_minutes * 60_000 {
            for segment in qrm.sends(run_ms, turn_sec, &settings()) {
                sends.push((run_ms, segment));
            }
            run_ms += whole_ms(turn_sec + quiet_sec);
        }
        sends
    }

    /// A station starts up about every four minutes, sends one to five
    /// messages and goes; it sends `QRL?`, a long CQ or your call and
    /// `QSY QSY`, at 30 to 49 WPM, and a message is never over another of
    /// its own.
    #[test]
    fn now_and_then_a_station_sends_on_your_frequency() {
        let sends = heard(1, 4.0, 1.0, 600);
        let firsts = sends
            .iter()
            .filter(|(_, segment)| !segment.text.starts_with("CQ CQ TEST"))
            .count();
        let stations = sends
            .iter()
            .filter(|(_, segment)| segment.text.contains("QSY") || segment.text.starts_with("QRL"))
            .count();
        assert_eq!(firsts, stations);
        // Six hundred minutes: about 155 stations, three messages each.
        assert!(
            (350..650).contains(&sends.len()),
            "{} messages",
            sends.len()
        );
        assert!(stations > 40, "{stations} stations began with QRL? or QSY");
        for (_, segment) in &sends {
            assert_eq!(segment.who, Sender::Qrm);
            let text = segment.text.as_str();
            assert!(
                ["QRL?", "QRL? QRL?", "DL1ABC QSY QSY"].contains(&text)
                    || text.starts_with("CQ CQ TEST "),
                "{text}"
            );
            assert!((30.0..=49.0).contains(&segment.voice.char_wpm));
            assert_eq!(segment.voice.char_wpm, segment.voice.effective_wpm);
        }
        // Each message in run time, none over the one before.
        let mut ended = 0.0;
        for (run_ms, segment) in &sends {
            let start = *run_ms as f64 / 1_000.0 + segment.start_sec;
            assert!(start >= ended, "a station sends over itself");
            ended = *run_ms as f64 / 1_000.0 + segment.end_sec;
        }
    }

    /// Stations start up as often as MorseRunner's, 15.5 an hour, whether
    /// or not one still sends, and each sends three messages on average, so
    /// a run hears 46.5 messages of QRM an hour.
    #[test]
    fn stations_start_up_as_often_as_morserunners() {
        const MINUTES: u64 = 8_000;
        let mut messages = 0;
        for seed in 0..5 {
            let mut qrm = wpx(seed);
            let mut run_ms = 0;
            while run_ms < MINUTES * 60_000 {
                messages += qrm.sends(run_ms, 5.0, &settings()).len();
                run_ms += 5_000;
            }
        }
        let per_hour = messages as f64 / (5.0 * MINUTES as f64 / 60.0);
        assert!(
            (per_hour - 46.5).abs() < 46.5 * 0.05,
            "{per_hour:.1} messages an hour"
        );
    }

    /// A message comes due inside a turn and is sent there, from when it is
    /// due; one due while nothing was on the air goes out as the next turn
    /// starts; and the same seed sends the same.
    #[test]
    fn qrm_is_heard_inside_turns() {
        let mut qrm = wpx(7);
        let due = qrm.next_ms;
        // A turn that ends before it is due hears nothing of it.
        assert!(qrm.sends(0, due as f64 / 2_000.0, &settings()).is_empty());
        // A turn around it hears it as it comes due.
        let mut around = qrm.clone();
        let sends = around.sends(due - 500, 1.0, &settings());
        assert_eq!(sends.len(), 1);
        assert!((sends[0].start_sec - 0.5).abs() < 1e-9);
        // A turn long after it hears it from its start.
        let sends = qrm.sends(due + 9_000, 1.0, &settings());
        assert_eq!(sends[0].start_sec, 0.0);
        assert_eq!(heard(5, 3.0, 2.0, 30), heard(5, 3.0, 2.0, 30));
    }

    /// After a long silence, of the stations that started up in it only
    /// the last is heard, as the next turn starts: the others came and
    /// went with nothing on the air, so no queue of them follows.
    #[test]
    fn a_long_silence_brings_one_station_not_a_queue() {
        for seed in 0..20 {
            let mut qrm = wpx(seed);
            let hour_ms = 3_600_000;
            let sends = qrm.sends(hour_ms, 1.0, &settings());
            assert_eq!(sends.len(), 1, "seed {seed}");
            assert_eq!(sends[0].start_sec, 0.0, "seed {seed}");
            assert!(qrm.next_ms >= hour_ms, "seed {seed}: a queue behind it");
        }
    }

    /// Its long CQ is the contest's, with its own call twice; with no call
    /// of yours to send, it sends no `QSY`.
    #[test]
    fn its_long_cq_is_the_contests() {
        let mut rng = SplitMix64::new(3);
        let mut station = QrmStation {
            call: "K5ABC".to_string(),
            voice: resolve_station(&settings(), &mut rng),
            fade_from_sec: 0.0,
            left: 1,
            first: false,
        };
        let rules = ContestKind::FieldDay.rules();
        assert_eq!(
            station.message(rules, "K1ABC", &mut rng),
            "CQ CQ FD K5ABC K5ABC FD"
        );
        for _ in 0..200 {
            station.first = true;
            assert!(!station.message(rules, "", &mut rng).contains("QSY"));
        }
    }
}
