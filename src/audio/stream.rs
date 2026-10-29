//! The receiver rendered ahead of the clock, a stretch at a time, and made
//! again from any moment: how the browser plays the very mixer the native
//! player runs.
//!
//! A browser plays what it is handed ahead of time, and nothing can run in
//! its audio thread sample by sample short of an AudioWorklet. So the mixer
//! is run ahead of the browser's clock, a tenth of a second at a time, each
//! stretch handed over to be played at its own moment, and a copy of the
//! mixer kept from the start of each. Whatever changes what is heard — a
//! send, a stop, a new band, the RF gain — is made at a moment just past
//! what the browser has taken already: the stretch that moment falls in is
//! played again from its copy up to there, sample for sample as it was, the
//! change made, and everything after it made anew. The band, the stations,
//! the receiver's filter and AGC, the mute while you send, the fades and the
//! fading are then the native player's own, not an imitation of them in
//! nodes.
//!
//! A stream that falls behind the clock — a page busy for a while — skips
//! what it should have played meanwhile, and a send whose sound it skipped
//! is told so ([`ReceiverStream::watch`]): its own clock ran on regardless,
//! and would say it was heard.

use std::cell::Cell;
use std::collections::VecDeque;
use std::rc::{Rc, Weak};

use super::mixer::{Mixer, Swept};

/// How long each stretch is: long enough that a browser busy for a moment
/// has several queued, short enough that one remade is little work.
pub const STRETCH_SEC: f64 = 0.1;

/// The most of a stall the mixer is run on over unheard, to keep its time
/// with the clock's ([`ReceiverStream::catch_up_to`]): the length of a
/// send or so, which the page makes up in a few tens of milliseconds even
/// in a browser on a phone. A longer stall is not made up at all: the
/// sends in flight are dropped rather than played that much late.
const CATCH_UP_MOST_SEC: f64 = 2.0;

/// The mixer, run ahead of a clock: what it has rendered, still to play,
/// each stretch with the node `N` it was handed over as. Everything is
/// counted in samples from the clock's moment the stream started at, so
/// stretches meet to the sample however long it runs.
pub struct ReceiverStream<N> {
    /// The mixer as it stands at `next`.
    mixer: Mixer,
    sample_rate: f64,
    stretch: usize,
    out: Vec<f32>,
    /// The clock's moment at sample 0.
    origin: f64,
    /// Where the next stretch starts.
    next: u64,
    queued: VecDeque<Queued<N>>,
    /// Nothing to play: the mixer is quiet and its last stretch was
    /// silence, so none is rendered until something changes.
    idle: bool,
    /// The sends whose sound is watched for being skipped, while anything
    /// still listens for it.
    watched: Vec<Watched>,
}

/// A send's sound, from the sample it starts at to the one it ends at, and
/// where to say that the stream skipped some of it.
struct Watched {
    starts: u64,
    ends: u64,
    missed: Weak<Cell<bool>>,
}

/// A send's word that the stream skipped some of its sound, falling behind
/// the clock while it played or waited to ([`ReceiverStream::watch`]).
#[derive(Clone, Debug, Default)]
pub struct Missed(Rc<Cell<bool>>);

impl Missed {
    /// Whether some of the send's sound was skipped.
    pub fn is_set(&self) -> bool {
        self.0.get()
    }
}

/// A stretch handed over, from sample `starts` to `ends`, and the mixer as
/// it stood before it.
struct Queued<N> {
    starts: u64,
    ends: u64,
    before: Mixer,
    node: N,
}

/// A stretch already handed over that a change takes back.
#[derive(Debug, PartialEq)]
pub enum Taken<N> {
    /// To stop at a moment, the rest of it made anew.
    StopAt(N, f64),
    /// Never to play.
    Never(N),
}

impl<N: Clone> ReceiverStream<N> {
    /// `mixer` from `now` on the clock it is played by.
    pub fn new(mixer: Mixer, now: f64) -> Self {
        let sample_rate = f64::from(mixer.sample_rate());
        let stretch = ((STRETCH_SEC * sample_rate).round() as usize).max(128);
        Self {
            mixer,
            sample_rate,
            stretch,
            out: vec![0.0; stretch],
            origin: now,
            next: 0,
            queued: VecDeque::new(),
            idle: false,
            watched: Vec::new(),
        }
    }

    /// The mixer as it stands at the end of what has been rendered.
    pub fn mixer(&self) -> &Mixer {
        &self.mixer
    }

    /// Render stretches until the next starts at `until` or later, each
    /// handed to `play` with the moment it starts: from `soonest` at the
    /// earliest, the soonest a stretch can still start on time, should the
    /// clock have run past what was rendered. A stretch `play` will not take
    /// is not rendered either.
    pub fn render_until<E>(
        &mut self,
        soonest: f64,
        until: f64,
        mut play: impl FnMut(f64, &[f32]) -> Result<N, E>,
    ) -> Result<(), E> {
        self.catch_up(self.sample_at(soonest));
        while !self.idle && self.time(self.next) < until {
            // What has finished goes before the copy is taken, so no copy
            // carries it.
            self.mixer.sweep(&mut Swept::new());
            let before = self.mixer.clone();
            let len = self.stretch;
            self.render(len);
            let node = match play(self.time(self.next), &self.out[..len]) {
                Ok(node) => node,
                Err(err) => {
                    // Played back to where it was.
                    self.mixer = before;
                    return Err(err);
                }
            };
            let ends = self.next + len as u64;
            self.queued.push_back(Queued {
                starts: self.next,
                ends,
                before,
                node,
            });
            self.next = ends;
            self.idle = self.mixer.is_quiet() && self.out[..len].iter().all(|s| *s == 0.0);
        }
        Ok(())
    }

    /// Make `change` at `at`, or as soon after it as anything is still to be
    /// rendered: what was rendered from there on is taken back, the stretch
    /// it falls in played again up to `at` exactly as it was. When the
    /// change was made, and what was taken back.
    pub fn change_at(
        &mut self,
        at: f64,
        change: impl FnOnce(&mut Mixer, &mut Swept),
    ) -> (f64, Vec<Taken<N>>) {
        let at = self.sample_at(at);
        let mut taken = Vec::new();
        if let Some(k) = self.queued.iter().position(|queued| queued.ends > at) {
            let mut later = self.queued.split_off(k);
            let first = later.pop_front().expect("one at k");
            let cut = at.max(first.starts);
            self.mixer = first.before.clone();
            self.next = first.starts;
            self.render((cut - first.starts) as usize);
            self.next = cut;
            if cut > first.starts {
                taken.push(Taken::StopAt(first.node.clone(), self.time(cut)));
                self.queued.push_back(Queued { ends: cut, ..first });
            } else {
                taken.push(Taken::Never(first.node));
            }
            taken.extend(later.into_iter().map(|queued| Taken::Never(queued.node)));
        } else {
            // Nothing rendered from `at` on: the mixer stands no later than
            // it, and is run on to there.
            self.catch_up(at);
        }
        self.idle = false;
        let mut swept = Swept::new();
        change(&mut self.mixer, &mut swept);
        drop(swept);
        (self.time(self.next), taken)
    }

    /// The stretches over by `now`, let go of.
    pub fn played_by(&mut self, now: f64) -> Vec<N> {
        let now = self.sample_at(now);
        let mut over = Vec::new();
        while self.queued.front().is_some_and(|queued| queued.ends <= now) {
            over.extend(self.queued.pop_front().map(|queued| queued.node));
        }
        over
    }

    /// Run the mixer on to the clock's moment `soonest`, should it have
    /// fallen behind it ([`Self::catch_up`]). Catching up takes time, so a
    /// caller reads the clock again after it before scheduling anything.
    pub fn catch_up_to(&mut self, soonest: f64) {
        self.catch_up(self.sample_at(soonest));
    }

    /// Watch a send's sound, from `starts` to `ends` on the clock, for the
    /// stream skipping any of it from now on ([`Self::catch_up`]).
    pub fn watch(&mut self, starts: f64, ends: f64) -> Missed {
        let missed = Missed::default();
        self.watched
            .retain(|watched| watched.missed.strong_count() > 0);
        self.watched.push(Watched {
            starts: self.sample_at(starts),
            ends: self.sample_at(ends),
            missed: Rc::downgrade(&missed.0),
        });
        missed
    }

    /// Fallen behind `soonest` — a long task, a hidden page — the stream
    /// picks up from there rather than playing into the past all at once.
    /// What should have played meanwhile is lost, but the mixer is run on
    /// over it unheard, so everything after it plays when it was meant to,
    /// and a send's own clock still says when it is heard; a send whose
    /// sound was in it is told it went unheard. A stall longer than
    /// [`CATCH_UP_MOST_SEC`] is not made up, rather than stall the page
    /// again making it up: the sends in flight, which would play on that
    /// much late, are dropped, and are told so whatever of them was still
    /// to come; the band picks up where it was.
    fn catch_up(&mut self, soonest: u64) {
        if self.next >= soonest {
            return;
        }
        if !self.idle {
            let most = (CATCH_UP_MOST_SEC * self.sample_rate) as u64;
            if soonest - self.next <= most {
                let mut left = (soonest - self.next) as usize;
                while left > 0 {
                    let len = left.min(self.stretch);
                    self.render(len);
                    left -= len;
                }
                self.skipped(self.next, soonest);
            } else {
                let mut swept = Swept::new();
                self.mixer.drop_sends(&mut swept);
                drop(swept);
                self.skipped(self.next, u64::MAX);
            }
        }
        self.next = soonest;
    }

    /// Nothing was heard from sample `from` to `until`: every send watched
    /// whose sound falls in it is told so.
    fn skipped(&mut self, from: u64, until: u64) {
        self.watched.retain(|watched| {
            let Some(missed) = watched.missed.upgrade() else {
                return false;
            };
            if watched.starts < until && watched.ends > from {
                missed.set(true);
            }
            true
        });
    }

    /// Whether nothing is being rendered, there being nothing to hear.
    pub fn is_idle(&self) -> bool {
        self.idle
    }

    /// The clock's moment at `sample`.
    fn time(&self, sample: u64) -> f64 {
        self.origin + sample as f64 / self.sample_rate
    }

    /// The sample nearest the clock's moment `at`, none before the first.
    fn sample_at(&self, at: f64) -> u64 {
        let samples = ((at - self.origin) * self.sample_rate).round();
        if samples.is_finite() && samples > 0.0 {
            samples as u64
        } else {
            0
        }
    }

    /// Run the mixer over `len` samples from `next` into `out`.
    fn render(&mut self, len: usize) {
        if self.out.len() < len {
            self.out.resize(len, 0.0);
        }
        let out = &mut self.out[..len];
        out.fill(0.0);
        self.mixer.render(out, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::mixer::{BandVoice, SendVoice};
    use crate::audio::shape::Gains;
    use cw_core::band::FilterDesign;
    use cw_core::{
        PlannedTransmission, SplitMix64, TrainingSettings, Transmission, plan_transmission,
        resolve_station,
    };

    const SR: u32 = 16_000;

    fn settings() -> TrainingSettings {
        let mut settings = TrainingSettings::default();
        settings.band.qrn_enabled = false;
        settings
    }

    fn plan(settings: &TrainingSettings, text: &str) -> PlannedTransmission {
        let voice = resolve_station(settings, &mut SplitMix64::new(5));
        plan_transmission(&Transmission::alone(text, voice), settings)
    }

    fn send(settings: &TrainingSettings, text: &str, at_sec: f64) -> SendVoice {
        let receiver = FilterDesign::from_settings(settings);
        SendVoice::new(&plan(settings, text), &receiver, None, SR, at_sec)
    }

    fn mixer(settings: &TrainingSettings) -> Mixer {
        let mut mixer = Mixer::new(SR);
        let mut swept = Swept::new();
        let band = BandVoice::for_settings(settings, SR, 7);
        mixer.set_band_at(band, Gains::of(settings), &mut swept);
        mixer.start(send(settings, "TEST", 0.0), &mut swept);
        mixer
    }

    /// What a browser plays of the stretches handed over, as the stream
    /// takes them back: each from its moment, cut short where a change
    /// stopped it, or not at all.
    #[derive(Default)]
    struct Browser {
        /// Each stretch: where it starts, in samples, its samples, and how
        /// many of them play.
        stretches: Vec<(usize, Vec<f32>, usize)>,
    }

    impl Browser {
        fn play(&mut self, starts: f64, samples: &[f32]) -> Result<usize, ()> {
            let at = (starts * f64::from(SR)).round() as usize;
            self.stretches.push((at, samples.to_vec(), samples.len()));
            Ok(self.stretches.len() - 1)
        }

        fn take_back(&mut self, taken: Vec<Taken<usize>>) {
            for taken in taken {
                match taken {
                    Taken::StopAt(i, at) => {
                        let at = (at * f64::from(SR)).round() as usize;
                        let stretch = &mut self.stretches[i];
                        stretch.2 = at - stretch.0;
                    }
                    Taken::Never(i) => self.stretches[i].2 = 0,
                }
            }
        }

        /// Everything played, from the start, to `samples` long.
        fn heard(&self, samples: usize) -> Vec<f32> {
            let mut out = vec![0.0f32; samples];
            let mut covered = vec![false; samples];
            for (at, stretch, plays) in &self.stretches {
                for (i, sample) in stretch[..*plays].iter().enumerate() {
                    if let Some(slot) = out.get_mut(at + i) {
                        assert!(!covered[at + i], "two stretches play at {}", at + i);
                        covered[at + i] = true;
                        *slot = *sample;
                    }
                }
            }
            out
        }
    }

    /// Fallen behind the clock — the page busy, or hidden — the stream picks
    /// up where the clock is, and what it plays from there is what the
    /// mixer plays there: the time in between is lost, not slipped into
    /// everything after it, so a send ends when its own clock says it does.
    #[test]
    fn a_stream_that_falls_behind_keeps_its_time() {
        let settings = settings();
        let mut stream = ReceiverStream::new(mixer(&settings), 0.0);
        let mut browser = Browser::default();
        stream
            .render_until(0.0, 0.3, |starts, samples| browser.play(starts, samples))
            .expect("played");
        stream
            .render_until(1.5, 2.0, |starts, samples| browser.play(starts, samples))
            .expect("played");
        let total = 2 * SR as usize;
        let heard = browser.heard(total);
        let mut straight = mixer(&settings);
        let mut expected = vec![0.0f32; total];
        straight.render(&mut expected, 0);
        let from = (1.5 * f64::from(SR)).round() as usize;
        assert!(heard[from..].iter().any(|s| *s != 0.0), "nothing played");
        let first_off = heard[from..]
            .iter()
            .zip(&expected[from..])
            .position(|(a, b)| a != b);
        assert_eq!(first_off, None, "the stream slipped behind the mixer");
    }

    /// Caught up to the clock before anything is scheduled, as the browser
    /// does, a stream picks up from the clock as it stands after catching
    /// up, which has moved on meanwhile, and still plays there what the
    /// mixer plays there; the time it took catching up is skipped as the
    /// stall before it was.
    #[test]
    fn a_stream_caught_up_first_plays_on_from_the_clock_after() {
        let settings = settings();
        let mut stream = ReceiverStream::new(mixer(&settings), 0.0);
        let mut browser = Browser::default();
        stream
            .render_until(0.0, 0.3, |starts, samples| browser.play(starts, samples))
            .expect("played");
        stream.catch_up_to(1.5);
        stream
            .render_until(1.6, 2.0, |starts, samples| browser.play(starts, samples))
            .expect("played");
        let total = 2 * SR as usize;
        let heard = browser.heard(total);
        let mut straight = mixer(&settings);
        let mut expected = vec![0.0f32; total];
        straight.render(&mut expected, 0);
        let from = (1.6 * f64::from(SR)).round() as usize;
        let first_off = heard[from..]
            .iter()
            .zip(&expected[from..])
            .position(|(a, b)| a != b);
        assert_eq!(first_off, None, "the stream slipped behind the mixer");
        assert!(
            heard[(0.3 * f64::from(SR)) as usize..from]
                .iter()
                .all(|s| *s == 0.0),
            "the stall was played"
        );
    }

    /// The sample nearest `seconds` in.
    fn at(seconds: f64) -> usize {
        (seconds * f64::from(SR)).round() as usize
    }

    /// The furthest a waveform swings from nothing.
    fn loudest(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0, |a, s| a.max(s.abs()))
    }

    /// A stall the stream makes up still loses what should have played in
    /// it: a send whose sound was there is told it went unheard, though its
    /// clock ran on and would say it was heard on time. One heard out before
    /// the stall, and one that starts after it, are not.
    #[test]
    fn a_send_in_a_stall_is_told_it_went_unheard() {
        let settings = settings();
        let mut stream = ReceiverStream::new(mixer(&settings), 0.0);
        let lookahead = stream.mixer().lookahead_sec();
        let sound = plan(&settings, "TEST").sound_end_sec();
        assert!(sound > 1.0, "TEST sounds across the stall: {sound}");
        let test = stream.watch(lookahead, lookahead + sound);
        let before = stream.watch(0.0, 0.25);
        let after = stream.watch(1.6, 1.9);
        let mut browser = Browser::default();
        stream
            .render_until(0.0, 0.3, |starts, samples| browser.play(starts, samples))
            .expect("played");
        assert!(!test.is_set(), "all of it heard so far");
        stream.catch_up_to(1.5);
        stream
            .render_until(1.5, 2.0, |starts, samples| browser.play(starts, samples))
            .expect("played");
        assert!(test.is_set(), "a second of TEST went unheard");
        assert!(!before.is_set(), "heard out before the stall");
        assert!(!after.is_set(), "starts after the stall");
    }

    /// A stall longer than the stream makes up leaves nothing in flight to
    /// play behind the clock. Run on over only part of the stall, a send
    /// would play the rest of it that much late, for good, and its clock
    /// would say it was over before its last element sounded; it is
    /// dropped instead, and told it went unheard.
    #[test]
    fn a_long_stall_leaves_nothing_to_play_behind_the_clock() {
        let settings = settings();
        let mut alone = Mixer::new(SR);
        let mut swept = Swept::new();
        alone.start(send(&settings, "TEST TEST", 0.0), &mut swept);
        drop(swept);
        let mut stream = ReceiverStream::new(alone, 0.0);
        let lookahead = stream.mixer().lookahead_sec();
        let sound = plan(&settings, "TEST TEST").sound_end_sec();
        assert!(sound > 2.5, "TEST TEST outlasts a stall made up: {sound}");
        let test = stream.watch(lookahead, lookahead + sound);
        let mut browser = Browser::default();
        stream
            .render_until(0.0, 0.5, |starts, samples| browser.play(starts, samples))
            .expect("played");
        stream.catch_up_to(4.0);
        assert!(test.is_set(), "TEST TEST went unheard from the stall on");
        stream
            .render_until(4.0, 6.0, |starts, samples| browser.play(starts, samples))
            .expect("played");
        let heard = browser.heard(at(6.0));
        assert!(loudest(&heard[..at(0.5)]) > 0.1, "heard before the stall");
        // The receiver's audio stage rings down for a moment from where
        // it was.
        let after = loudest(&heard[at(4.05)..]);
        assert!(
            after < 1e-6,
            "TEST TEST played on behind the clock: {after}"
        );
        assert!(stream.is_idle(), "nothing left to play");
    }

    /// A send waiting through a long stall to start, a contest's
    /// continuation, would start that much late as well: it is dropped
    /// and told so. A send made after the stall plays when it was made
    /// to, and is not told anything.
    #[test]
    fn a_long_stall_drops_a_send_waiting_through_it_and_not_one_made_after() {
        let settings = settings();
        let lookahead = Mixer::new(SR).lookahead_sec();
        let dit = plan(&settings, "E").sound_end_sec();
        let mut waiting = Mixer::new(SR);
        let mut swept = Swept::new();
        waiting.start_at(send(&settings, "E", 5.0), None, at(5.0), &mut swept);
        let mut stream = ReceiverStream::new(waiting, 0.0);
        let waited = stream.watch(5.0 + lookahead, 5.0 + lookahead + dit);
        let mut browser = Browser::default();
        stream
            .render_until(0.0, 0.5, |starts, samples| browser.play(starts, samples))
            .expect("played");
        stream.catch_up_to(4.0);
        assert!(waited.is_set(), "it would have started late");
        stream
            .render_until(4.0, 8.0, |starts, samples| browser.play(starts, samples))
            .expect("played");
        let heard = browser.heard(at(8.0));
        let late = loudest(&heard[at(4.0)..]);
        assert!(late == 0.0, "the dit played behind the clock: {late}");

        let (made, _) = stream.change_at(8.0, |mixer, swept| {
            mixer.start(send(&settings, "E", 8.0), swept);
        });
        assert_eq!(made, 8.0);
        let fresh = stream.watch(made + lookahead, made + lookahead + dit);
        stream
            .render_until(8.0, 9.0, |starts, samples| browser.play(starts, samples))
            .expect("played");
        assert!(!fresh.is_set(), "made after the stall");
        let heard = browser.heard(at(9.0));
        // A stretch at a time, as the stream renders it, so it settles
        // to silence where the stream does.
        let mut straight = Mixer::new(SR);
        straight.start(send(&settings, "E", 8.0), &mut swept);
        let mut expected = vec![0.0f32; at(1.0)];
        for stretch in expected.chunks_mut(at(STRETCH_SEC)) {
            straight.render(stretch, 0);
        }
        assert!(loudest(&expected) > 0.1, "the dit sounds");
        let first_off = heard[at(8.0)..]
            .iter()
            .zip(&expected)
            .position(|(a, b)| a != b);
        assert_eq!(first_off, None, "the dit made after the stall, on time");
    }

    /// A change made mid-stretch, the queue a good way ahead of it, plays
    /// exactly as the mixer would have played with the change made at that
    /// sample in the first place: the stretch it falls in played again to
    /// there as it was, no sample twice and none missing, and the rest made
    /// anew.
    #[test]
    fn a_change_mid_stretch_plays_as_if_made_there() {
        let settings = settings();
        let at = 0.5373;
        let changed = |mixer: &mut Mixer, swept: &mut Swept| {
            mixer.start_at(send(&settings, "EEE", at), None, 0, swept);
        };
        let mut stream = ReceiverStream::new(mixer(&settings), 0.0);
        let mut browser = Browser::default();
        stream
            .render_until(0.0, 1.0, |starts, samples| browser.play(starts, samples))
            .expect("played");
        let (made, taken) = stream.change_at(at, changed);
        let cut = (at * f64::from(SR)).round() as usize;
        assert_eq!((made * f64::from(SR)).round() as usize, cut);
        assert!(
            matches!(taken.first(), Some(Taken::StopAt(..))),
            "{taken:?}"
        );
        browser.take_back(taken);
        stream
            .render_until(0.0, 2.0, |starts, samples| browser.play(starts, samples))
            .expect("played");
        let total = 2 * SR as usize;
        let heard = browser.heard(total);

        let mut straight = mixer(&settings);
        let mut expected = vec![0.0f32; total];
        straight.render(&mut expected[..cut], 0);
        let mut swept = Swept::new();
        changed(&mut straight, &mut swept);
        straight.render(&mut expected[cut..], 0);
        let first_off = heard.iter().zip(&expected).position(|(a, b)| a != b);
        assert_eq!(first_off, None, "the stream parts from the mixer there");
        assert!(
            heard[cut..].iter().any(|s| *s != 0.0),
            "nothing after the change"
        );
    }

    /// A change before anything is queued from its moment picks up there;
    /// one at the start of a stretch takes that stretch back whole.
    #[test]
    fn a_change_between_stretches_takes_them_back_whole() {
        let settings = settings();
        let mut stream = ReceiverStream::new(mixer(&settings), 0.0);
        let mut browser = Browser::default();
        stream
            .render_until(0.0, 0.5, |starts, samples| browser.play(starts, samples))
            .expect("played");
        let (made, taken) = stream.change_at(0.2, |mixer, _| mixer.stop());
        assert_eq!(made, 0.2);
        assert_eq!(
            taken,
            [Taken::Never(2), Taken::Never(3), Taken::Never(4)],
            "from the third stretch on"
        );
        let (made, taken) = stream.change_at(9.0, |mixer, _| mixer.stop());
        assert_eq!(
            (made, taken),
            (9.0, Vec::new()),
            "nothing there to take back"
        );
    }

    /// Once there is nothing left to hear, nothing more is rendered, until
    /// something changes; and what has played is let go of.
    #[test]
    fn a_quiet_receiver_renders_nothing_until_woken() {
        let mut quiet = settings();
        quiet.band.noise_enabled = false;
        quiet.band.agc_enabled = false;
        let mut stream = ReceiverStream::new(Mixer::new(SR), 0.0);
        assert!(stream.mixer().is_quiet());
        let mut browser = Browser::default();
        stream
            .render_until(0.0, 1.0, |starts, samples| browser.play(starts, samples))
            .expect("played");
        assert!(stream.is_idle());
        assert_eq!(browser.stretches.len(), 1, "one stretch of silence");
        let (made, _) = stream.change_at(3.0, |mixer, swept| {
            mixer.start(send(&quiet, "E", 3.0), swept);
        });
        assert_eq!(made, 3.0);
        stream
            .render_until(3.0, 3.5, |starts, samples| browser.play(starts, samples))
            .expect("played");
        assert_eq!(browser.stretches[1].0, 3 * SR as usize, "from the change");
        assert!(
            browser.stretches.len() > 2,
            "the dit and what rings on after it"
        );
        assert!(stream.is_idle(), "quiet again once it has rung out");
        assert_eq!(stream.played_by(3.15), [0, 1]);
    }

    /// A session over, its send stopped and its band taken away, the stream
    /// goes quiet once the band has faded and the send's release has run
    /// out, though no change comes after to sweep either out.
    #[test]
    fn a_band_taken_away_leaves_the_stream_to_go_quiet() {
        let settings = settings();
        let mut stream = ReceiverStream::new(mixer(&settings), 0.0);
        let mut browser = Browser::default();
        stream
            .render_until(0.0, 1.0, |starts, samples| browser.play(starts, samples))
            .expect("played");
        assert!(!stream.is_idle());
        stream.change_at(1.0, |mixer, swept| {
            mixer.stop();
            mixer.set_band(None, swept);
        });
        stream
            .render_until(1.0, 2.0, |starts, samples| browser.play(starts, samples))
            .expect("played");
        assert!(stream.is_idle(), "the band faded, the send released");
    }
}
