//! The paddle's sidetone: an oscillator the audio callback runs, keyed by
//! the UI without a lock, each edge sounded a constant lead after the
//! instant the keyer meant it for.
//!
//! The callback renders a whole buffer in microseconds, so a gate it only
//! read would open at the start of whichever buffer came next: every
//! element, and every space, a whole number of buffers long, a 40 ms dit
//! played as 30 or 50 on a 10 ms buffer. Instead each edge carries its
//! instant on a clock both sides read, and the callback, which knows when
//! each of its frames is due, opens or closes the gate at the frame that
//! instant falls on, a lead later: the longest it has lately been asked to
//! render at once, so the edge is always in hand before its frame is, and
//! the time a late timer takes on top.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use crate::audio::ToneRequest;
use crate::audio::tone::SIDETONE_LEAD_MARGIN_SEC;

/// Seconds on the one clock the UI and the audio callback both read, from
/// whenever it was first read.
pub fn clock_sec() -> f64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// How many edges can wait for the callback. It takes each a lead after it
/// was keyed, a few milliseconds, so two or three wait at most; more wait
/// only while the stream is closed, and then only the last matters.
const EDGES: usize = 16;

/// Each edge is one word: which edge it is in the order they were keyed,
/// so one written over as it was read is told from the one wanted; its
/// instant, in microseconds on [`clock_sec`]'s clock, enough for three
/// months; and on or off.
const SEQ_SHIFT: u32 = 44;
const SEQ_MASK: u64 = (1 << (64 - SEQ_SHIFT)) - 1;
const AT_MASK: u64 = (1 << (SEQ_SHIFT - 1)) - 1;

fn pack(seq: usize, on: bool, at_sec: f64) -> u64 {
    let at_us = ((at_sec.max(0.0) * 1e6) as u64).min(AT_MASK);
    ((seq as u64 & SEQ_MASK) << SEQ_SHIFT) | (at_us << 1) | u64::from(on)
}

fn unpack(word: u64) -> (u64, bool, f64) {
    let at_us = (word >> 1) & AT_MASK;
    (word >> SEQ_SHIFT, word & 1 == 1, at_us as f64 / 1e6)
}

/// The sidetone's pitch, level and edge shape, and the edges themselves,
/// as the UI keys them. Atomics only, so a squeeze never waits on the
/// callback. Keyed from one thread at a time, the player's.
pub struct SidetoneControl {
    hz_bits: AtomicU32,
    gain_bits: AtomicU32,
    rise_bits: AtomicU32,
    smoothing_bits: AtomicU32,
    edges: [AtomicU64; EDGES],
    /// How many edges have been keyed, ever: the next one's place.
    written: AtomicUsize,
    /// Whether the last edge keyed was on.
    keyed: AtomicBool,
}

impl SidetoneControl {
    pub fn new() -> Self {
        let defaults = ToneRequest::new(false, 600.0, 0.0);
        Self {
            hz_bits: AtomicU32::new(defaults.hz.to_bits()),
            gain_bits: AtomicU32::new(0.0f32.to_bits()),
            rise_bits: AtomicU32::new(defaults.rise_sec.to_bits()),
            smoothing_bits: AtomicU32::new(defaults.smoothing.to_bits()),
            edges: std::array::from_fn(|_| AtomicU64::new(0)),
            written: AtomicUsize::new(0),
            keyed: AtomicBool::new(false),
        }
    }

    /// Play `tone`: its pitch, level and edge from now on, and the gate
    /// opened or closed at the edge it was keyed for, `tone.ago_sec` ago.
    pub fn set(&self, tone: ToneRequest) {
        self.set_at(tone, clock_sec() - tone.ago_sec);
    }

    /// [`Self::set`], the edge keyed at `at_sec` on [`clock_sec`]'s clock.
    pub(super) fn set_at(&self, tone: ToneRequest, at_sec: f64) {
        self.hz_bits.store(tone.hz.to_bits(), Ordering::Relaxed);
        self.gain_bits.store(tone.gain.to_bits(), Ordering::Relaxed);
        self.rise_bits
            .store(tone.rise_sec.to_bits(), Ordering::Relaxed);
        self.smoothing_bits
            .store(tone.smoothing.to_bits(), Ordering::Relaxed);
        self.key(tone.on, at_sec);
    }

    /// Close the gate now.
    pub fn off(&self) {
        self.key(false, clock_sec());
    }

    /// An edge: the gate on or off at `at_sec`. One that changes nothing is
    /// not written, so it can never push out one still to sound.
    fn key(&self, on: bool, at_sec: f64) {
        if self.keyed.swap(on, Ordering::Relaxed) == on {
            return;
        }
        let seq = self.written.load(Ordering::Relaxed);
        self.edges[seq % EDGES].store(pack(seq, on, at_sec), Ordering::Release);
        self.written.store(seq + 1, Ordering::Release);
    }

    fn load(bits: &AtomicU32) -> f32 {
        f32::from_bits(bits.load(Ordering::Relaxed))
    }
}

/// How fast the shared clock and the stream's frames may drift apart, as a
/// share of the time gone by. Sound cards keep time to a hundred parts in a
/// million or so; this is five times that.
const DRIFT: f64 = 5e-4;

/// How quickly a long buffer, once rendered, stops counting towards the
/// lead: forgotten by e every this long, so the first, which fills the
/// device's whole buffer, or one after a stall, is forgotten within a
/// second or two, and buffers as long as the device always asks for never
/// are.
const LONGEST_FORGET_SEC: f64 = 0.5;

/// How long the key has to be quiet before the lead is let change: a new
/// lead mid-letter would move every edge after it.
const QUIET_SEC: f64 = 0.5;

/// The latest the first edge after a quiet spell can come and still set
/// the time for the rest: a stream woken by a squeeze plays its first
/// frame some milliseconds after it, and the edges after it keep their
/// distance from it rather than land on time without it.
const LATE_START_MAX_SEC: f64 = 0.1;

/// Where the stream's frames stand on the shared clock.
struct StreamClock {
    /// When frame 0 was due: the soonest any callback has come, less the
    /// frames already played before it, crept on as the clocks drift. A
    /// callback is only ever late, never early, so the soonest is when its
    /// frames were really asked for.
    origin: f64,
    /// When the last callback came.
    last: f64,
    /// The longest buffer lately rendered, in seconds.
    longest: f64,
    /// How long after its instant an edge sounds: the longest buffer and
    /// the margin, or, through a burst of keying that began late, as late
    /// as it began.
    lead: f64,
}

/// The oscillator and its gate, owned by the audio callback.
pub struct Sidetone {
    phase: f32,
    /// How far through its edge the gate stands: 0 shut, 1 open.
    progress: f64,
    /// Which way the gate is going: open, or shut.
    keyed: bool,
    sample_rate: f64,
    /// Frames rendered since the stream opened.
    played: u64,
    clock: Option<StreamClock>,
    /// The next edge keyed, read and not yet reached: on or off, and its
    /// instant.
    pending: Option<(bool, f64)>,
    /// The next edge to read; on the first callback, the oldest still kept.
    taken: Option<usize>,
    /// The instant of the last edge sounded.
    last_edge: f64,
}

impl Sidetone {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            phase: 0.0,
            progress: 0.0,
            keyed: false,
            sample_rate: f64::from(sample_rate.max(1)),
            played: 0,
            clock: None,
            pending: None,
            taken: None,
            last_edge: f64::NEG_INFINITY,
        }
    }

    /// Add the sidetone to a buffer of mono frames, for a callback that came
    /// at `now` on [`clock_sec`]'s clock.
    pub fn add_to(&mut self, out: &mut [f32], control: &SidetoneControl, now: f64) {
        let frames = out.len();
        self.observe(frames, now);
        let hz = f64::from(SidetoneControl::load(&control.hz_bits));
        let gain = SidetoneControl::load(&control.gain_bits);
        let rise = f64::from(SidetoneControl::load(&control.rise_bits)).max(1e-4);
        let smoothing = f64::from(SidetoneControl::load(&control.smoothing_bits)).clamp(0.0, 1.0);
        let edge = Edge {
            step: (std::f64::consts::TAU * hz / self.sample_rate) as f32,
            gain,
            slope: 1.0 / (rise * self.sample_rate),
            smoothing,
        };
        let mut from = 0;
        while from < frames {
            match self.next_edge(control, from, frames) {
                Some((at, on)) => {
                    self.render(&mut out[from..at], &edge);
                    self.keyed = on;
                    from = at;
                }
                None => {
                    self.render(&mut out[from..], &edge);
                    break;
                }
            }
        }
        self.played += frames as u64;
    }

    /// Whether the gate is shut and has been since `QUIET_SEC` before
    /// `now`, with no edge waiting.
    fn quiet_at(&self, now: f64) -> bool {
        !self.keyed && self.progress == 0.0 && now - self.last_edge >= QUIET_SEC
    }

    /// Take the measure of a callback of `frames` that came at `now`.
    fn observe(&mut self, frames: usize, now: f64) {
        let quiet = self.pending.is_none() && self.quiet_at(now);
        let due = now - self.played as f64 / self.sample_rate;
        let length = frames as f64 / self.sample_rate;
        let Some(clock) = self.clock.as_mut() else {
            // The first callback fills the device's whole buffer, the most
            // it will ever be asked for at once, likely as not.
            self.clock = Some(StreamClock {
                origin: due,
                last: now,
                longest: length,
                lead: length + SIDETONE_LEAD_MARGIN_SEC,
            });
            return;
        };
        let since = (now - clock.last).max(0.0);
        clock.last = now;
        clock.origin = (clock.origin + since * DRIFT).min(due);
        clock.longest = length.max(clock.longest * (-since / LONGEST_FORGET_SEC).exp());
        // Only between bursts of keying: an edge that comes too late for a
        // lead too short sounds as soon as it can.
        if quiet {
            clock.lead = clock.longest + SIDETONE_LEAD_MARGIN_SEC;
        }
    }

    /// The next edge to fall in this buffer, on or after frame `from` and
    /// before `frames`: where, and which way. One already overdue falls at
    /// `from`, as soon as it can.
    fn next_edge(
        &mut self,
        control: &SidetoneControl,
        from: usize,
        frames: usize,
    ) -> Option<(usize, bool)> {
        let (on, at) = match self.pending.take() {
            Some(edge) => edge,
            None => self.read(control)?,
        };
        let starts_burst = self.quiet_at(at);
        let sample_rate = self.sample_rate;
        let clock = self.clock.as_mut()?;
        let mut frame =
            ((at + clock.lead - clock.origin) * sample_rate).round() - self.played as f64;
        let overdue = (from as f64 - frame) / sample_rate;
        if overdue > 0.0 && starts_burst && overdue <= LATE_START_MAX_SEC {
            // The first edge of a burst, late: the burst keeps its time from
            // here, as late as it began.
            clock.lead += overdue;
            frame = from as f64;
        }
        if frame >= frames as f64 {
            self.pending = Some((on, at));
            return None;
        }
        self.last_edge = at;
        Some(((frame.max(0.0) as usize).max(from), on))
    }

    /// The next edge keyed and not yet read. Should the UI have keyed so
    /// many while the stream was not playing that the one wanted has been
    /// written over, the newest is taken: what came before it is long past.
    fn read(&mut self, control: &SidetoneControl) -> Option<(bool, f64)> {
        let written = control.written.load(Ordering::Acquire);
        let taken = self.taken.get_or_insert(written.saturating_sub(EDGES));
        if *taken >= written {
            return None;
        }
        if written - *taken > EDGES {
            // Written over already: the newest, and what came before it is
            // long past.
            *taken = written - 1;
        }
        let word = control.edges[*taken % EDGES].load(Ordering::Acquire);
        let (seq, on, at) = unpack(word);
        if seq != *taken as u64 & SEQ_MASK {
            // Written over as it was read: the newest, next buffer.
            *taken = written - 1;
            return None;
        }
        *taken += 1;
        Some((on, at))
    }

    /// Play `out` with the gate going whichever way it is going.
    fn render(&mut self, out: &mut [f32], edge: &Edge) {
        let tau = std::f32::consts::TAU;
        let target = if self.keyed { 1.0 } else { 0.0 };
        for slot in out {
            if self.progress != target {
                self.progress = if self.keyed {
                    (self.progress + edge.slope).min(1.0)
                } else {
                    (self.progress - edge.slope).max(0.0)
                };
            }
            if self.progress == 0.0 {
                // Shut: nothing to work out until the next squeeze, which
                // starts the tone from the top of its cycle.
                self.phase = 0.0;
                continue;
            }
            let env = if self.progress == 1.0 {
                1.0
            } else {
                cw_core::edge_gain(self.progress, edge.smoothing) as f32
            };
            *slot += self.phase.sin() * env * edge.gain;
            self.phase += edge.step;
            if self.phase >= tau {
                self.phase -= tau;
            }
        }
    }
}

/// What one buffer plays with: the oscillator's step a frame, its level,
/// how far the gate moves a frame, and the shape it moves along.
struct Edge {
    step: f32,
    gain: f32,
    slope: f64,
    smoothing: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    /// Callbacks of `frames` each, from `start` on the shared clock, every
    /// one on time: what `sidetone` plays over `seconds`.
    fn play(
        sidetone: &mut Sidetone,
        control: &SidetoneControl,
        start: f64,
        frames: usize,
        seconds: f64,
    ) -> Vec<f32> {
        let total = (seconds * f64::from(SR)) as usize;
        let mut out = vec![0.0f32; total];
        for (k, chunk) in out.chunks_mut(frames).enumerate() {
            let now = start + (k * frames) as f64 / f64::from(SR);
            sidetone.add_to(chunk, control, now);
        }
        out
    }

    /// The frames the tone is at least half up over, as runs: where each
    /// begins and how long it is. Read off the loudest of each cycle, so a
    /// run is the same few frames longer than its mark, whatever its length.
    fn marks(out: &[f32], gain: f32) -> Vec<(usize, usize)> {
        // Half up, by the envelope, wherever in its cycle the tone is: the
        // loudest of each stretch a little over one cycle long.
        let cycle = 96;
        let up: Vec<bool> = (0..out.len())
            .map(|i| {
                let lo = i.saturating_sub(cycle / 2);
                let hi = (i + cycle / 2).min(out.len());
                out[lo..hi].iter().fold(0.0f32, |a, s| a.max(s.abs())) >= gain * 0.5
            })
            .collect();
        let mut runs = Vec::new();
        let mut start = None;
        for (i, on) in up.iter().enumerate() {
            match (on, start) {
                (true, None) => start = Some(i),
                (false, Some(s)) => {
                    runs.push((s, i - s));
                    start = None;
                }
                _ => {}
            }
        }
        runs
    }

    /// A squeeze opens the gate along the keying envelope's edge, without
    /// a step, at the pitch and level asked for; letting go closes it the
    /// same way and leaves nothing behind.
    #[test]
    fn the_sidetone_follows_the_key_without_a_click() {
        let control = SidetoneControl::new();
        let mut sidetone = Sidetone::new(SR);
        let mut out = vec![0.0f32; 64];
        sidetone.add_to(&mut out, &control, 0.0);
        assert!(out.iter().all(|s| *s == 0.0));

        let tone = ToneRequest::new(true, 500.0, 0.5).shaped(0.005, 0.75);
        control.set_at(tone, 0.01);
        control.set_at(ToneRequest { on: false, ..tone }, 0.11);
        let keyed = play(&mut sidetone, &control, 0.002, 256, 0.3);
        let peak = keyed.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!((peak - 0.5).abs() < 0.01, "peak {peak}");
        let biggest = keyed
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        // A 500 Hz sine at 0.5 moves at most 0.033 a sample at 48 kHz.
        assert!(biggest < 0.036, "a step of {biggest}");
        assert!(
            keyed[keyed.len() - 4_800..].iter().all(|s| *s == 0.0),
            "it never shut"
        );
    }

    /// The sidetone adds to what is already there.
    #[test]
    fn the_sidetone_mixes_rather_than_overwrites() {
        let control = SidetoneControl::new();
        let mut sidetone = Sidetone::new(SR);
        let mut out = vec![0.25f32; 16];
        sidetone.add_to(&mut out, &control, 0.0);
        assert!(out.iter().all(|s| *s == 0.25));
    }

    /// Each element and each space sounds as long as it was keyed, to the
    /// frame, however long a stretch the output renders at once: 60 ms dits
    /// a dit apart, through buffers of 37, 256, 480 and 1024 frames. Where
    /// the gate only followed the key, they were whole buffers long.
    #[test]
    fn elements_and_spaces_are_as_long_as_they_were_keyed() {
        let dit = 0.06;
        for frames in [37, 256, 480, 1_024] {
            let control = SidetoneControl::new();
            let mut sidetone = Sidetone::new(SR);
            let tone = ToneRequest::new(true, 600.0, 0.4).shaped(0.004, 0.75);
            // Keyed 3 ms into the stream's life, whose first buffer is no
            // longer than the rest, and every edge after that.
            for k in 0..8 {
                let at = 0.5 + k as f64 * dit;
                control.set_at(
                    ToneRequest {
                        on: k % 2 == 0,
                        ..tone
                    },
                    at,
                );
            }
            let out = play(&mut sidetone, &control, 0.497, frames, 1.5);
            let marks = marks(&out, 0.4);
            assert_eq!(marks.len(), 4, "{frames} frames: {marks:?}");
            let dit_frames = (dit * f64::from(SR)) as usize;
            assert!(marks[0].1.abs_diff(dit_frames) < 96, "{frames}: {marks:?}");
            for pair in marks.windows(2) {
                assert!(pair[1].1.abs_diff(pair[0].1) <= 1, "{frames}: {marks:?}");
                assert!(
                    (pair[1].0 - pair[0].0).abs_diff(2 * dit_frames) <= 1,
                    "{frames} frames: {marks:?}"
                );
            }
        }
    }

    /// An edge keyed long ago sounds as soon as it can, rather than never.
    #[test]
    fn an_edge_long_overdue_sounds_at_once() {
        let control = SidetoneControl::new();
        let mut sidetone = Sidetone::new(SR);
        let mut warm = vec![0.0f32; 480];
        for k in 0..20 {
            sidetone.add_to(&mut warm, &control, 1.0 + k as f64 * 0.01);
        }
        control.set_at(ToneRequest::new(true, 600.0, 0.4), 0.0);
        let mut out = vec![0.0f32; 480];
        sidetone.add_to(&mut out, &control, 1.2);
        assert!(out[..48].iter().any(|s| *s != 0.0), "it waited");
    }

    /// A burst of keying that reaches the stream late — the squeeze that
    /// woke it, before it played a frame — keeps its own time all the same,
    /// as late as it began; and the next burst, after a quiet spell, is no
    /// later than the stream's own lead.
    #[test]
    fn a_burst_begun_late_keeps_its_time() {
        let control = SidetoneControl::new();
        let mut sidetone = Sidetone::new(SR);
        let tone = ToneRequest::new(true, 600.0, 0.4).shaped(0.004, 0.75);
        // A dit at 0.5, its space, a dah; the stream starts 30 ms later.
        for (on, at) in [(true, 0.5), (false, 0.56), (true, 0.62), (false, 0.8)] {
            control.set_at(ToneRequest { on, ..tone }, at);
        }
        let out = play(&mut sidetone, &control, 0.53, 480, 0.5);
        let dit = (0.06 * f64::from(SR)) as usize;
        let first = marks(&out, 0.4);
        assert_eq!(first.len(), 2, "{first:?}");
        assert!(first[0].1.abs_diff(dit) < 96, "{first:?}");
        assert!(
            (first[1].0 - first[0].0).abs_diff(2 * dit) <= 1,
            "{first:?}"
        );
        assert!(
            (first[1].1 - first[0].1).abs_diff(2 * dit) <= 2,
            "{first:?}"
        );
        // Quiet for a second, then a dit keyed on time is played the
        // stream's lead after it: one buffer and the margin.
        let quiet = play(&mut sidetone, &control, 1.03, 480, 0.57);
        assert!(quiet.iter().all(|s| *s == 0.0));
        control.set_at(ToneRequest { on: true, ..tone }, 1.6);
        control.set_at(ToneRequest { on: false, ..tone }, 1.66);
        let later = play(&mut sidetone, &control, 1.6, 480, 0.2);
        let then = marks(&later, 0.4);
        assert_eq!(then.len(), 1, "{then:?}");
        let lead = 480.0 / f64::from(SR) + SIDETONE_LEAD_MARGIN_SEC;
        let expected = (lead * f64::from(SR)) as usize;
        // Half up a little over half the edge after it opens.
        assert!(
            then[0].0.abs_diff(expected) <= 150,
            "{then:?} against {expected}"
        );
    }

    /// The first buffer, the device's whole, and one after a stall are soon
    /// forgotten: a squeeze two seconds on is played the lead of the
    /// buffers the device asks for now, not of those.
    #[test]
    fn a_long_buffer_is_soon_forgotten() {
        let control = SidetoneControl::new();
        let mut sidetone = Sidetone::new(SR);
        let mut first = vec![0.0f32; 14_400];
        sidetone.add_to(&mut first, &control, 1.0);
        let quiet = play(&mut sidetone, &control, 1.3, 480, 2.0);
        assert!(quiet.iter().all(|s| *s == 0.0));
        control.set_at(ToneRequest::new(true, 600.0, 0.4).shaped(0.004, 0.75), 3.3);
        let keyed = play(&mut sidetone, &control, 3.3, 480, 0.1);
        let opened = keyed.iter().position(|s| *s != 0.0).expect("it sounded");
        let lead = 480.0 / f64::from(SR) + SIDETONE_LEAD_MARGIN_SEC;
        assert!(
            opened.abs_diff((lead * f64::from(SR)) as usize) <= 1,
            "opened {opened} frames on"
        );
    }

    /// A gate opened and closed faster than its edge turns back where it
    /// stands, never stepping, and closes in full.
    #[test]
    fn a_tap_shorter_than_the_edge_never_steps() {
        let control = SidetoneControl::new();
        let mut sidetone = Sidetone::new(SR);
        let tone = ToneRequest::new(true, 500.0, 0.5).shaped(0.008, 1.0);
        control.set_at(tone, 0.01);
        control.set_at(ToneRequest { on: false, ..tone }, 0.012);
        let out = play(&mut sidetone, &control, 0.0, 128, 0.1);
        let biggest = out
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(biggest < 0.036, "a step of {biggest}");
        let peak = out.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(peak > 0.0 && peak < 0.25, "a 2 ms tap reaches {peak}");
        assert!(out[2_400..].iter().all(|s| *s == 0.0));
    }

    /// Keyed so often that the edges wrap round while nothing plays them,
    /// the gate ends up where the last one left it.
    #[test]
    fn edges_keyed_while_nothing_plays_leave_the_last_word() {
        let control = SidetoneControl::new();
        let tone = ToneRequest::new(true, 600.0, 0.4);
        for k in 0..(EDGES * 3 + 1) {
            control.set_at(
                ToneRequest {
                    on: k % 2 == 0,
                    ..tone
                },
                k as f64 * 0.001,
            );
        }
        let mut sidetone = Sidetone::new(SR);
        let out = play(&mut sidetone, &control, 1.0, 256, 0.1);
        assert!(
            out[2_400..].iter().any(|s| *s != 0.0),
            "the last edge was on"
        );
        assert_eq!(sidetone.taken, Some(EDGES * 3 + 1));
    }

    /// An edge that changes nothing is not written: it could only push out
    /// one still to sound.
    #[test]
    fn an_edge_that_changes_nothing_is_not_kept() {
        let control = SidetoneControl::new();
        control.set(ToneRequest::new(false, 600.0, 0.4));
        control.off();
        assert_eq!(control.written.load(Ordering::Relaxed), 0);
        control.set(ToneRequest::new(true, 600.0, 0.4));
        control.set(ToneRequest::new(true, 700.0, 0.4));
        assert_eq!(control.written.load(Ordering::Relaxed), 1);
        assert_eq!(SidetoneControl::load(&control.hz_bits), 700.0);
    }

    /// The gate opens as the keying envelope rises: its 10-90% rise is most
    /// of the edge asked for, and the click its edges make above 1.2 kHz is
    /// 50 dB under the tone, where the old half-millisecond gate's was 33.
    #[test]
    fn the_gate_opens_cleanly() {
        let control = SidetoneControl::new();
        let mut sidetone = Sidetone::new(SR);
        let tone = ToneRequest::new(true, 450.0, 0.5).shaped(0.006, 0.75);
        control.set_at(tone, 0.01);
        control.set_at(ToneRequest { on: false, ..tone }, 0.07);
        let out = play(&mut sidetone, &control, 0.0, 256, 0.2);
        let env: Vec<f32> = (0..out.len())
            .map(|i| {
                let lo = i.saturating_sub(53);
                let hi = (i + 53).min(out.len());
                out[lo..hi].iter().fold(0.0f32, |a, s| a.max(s.abs())) / 0.5
            })
            .collect();
        let first = |level: f32| env.iter().position(|e| *e >= level).expect("it rose");
        let rise = (first(0.9) - first(0.1)) as f64 / f64::from(SR);
        assert!(rise >= 0.003, "a 10-90% rise of {rise}");

        let click = click_db(&out);
        assert!(click < -50.0, "edges {click:.1} dB under the tone");
    }

    /// How loud the worst 2 ms of `out` is above 1.2 kHz, against the tone
    /// at its level of 0.5: through four Butterworth high-pass sections,
    /// which leave nothing of a tone at 450 Hz to speak of.
    fn click_db(out: &[f32]) -> f64 {
        let sr = f64::from(SR);
        let w = std::f64::consts::TAU * 1_200.0 / sr;
        let alpha = w.sin() / std::f64::consts::SQRT_2;
        let a0 = 1.0 + alpha;
        let b = [
            (1.0 + w.cos()) / 2.0 / a0,
            -(1.0 + w.cos()) / a0,
            (1.0 + w.cos()) / 2.0 / a0,
        ];
        let a = [-2.0 * w.cos() / a0, (1.0 - alpha) / a0];
        let mut signal: Vec<f64> = out.iter().map(|s| f64::from(*s)).collect();
        for _ in 0..4 {
            let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
            for s in &mut signal {
                let x = *s;
                let y = b[0] * x + b[1] * x1 + b[2] * x2 - a[0] * y1 - a[1] * y2;
                (x2, x1, y2, y1) = (x1, x, y1, y);
                *s = y;
            }
        }
        let window = (0.002 * sr) as usize;
        let worst = signal
            .windows(window)
            .map(|w| (w.iter().map(|s| s * s).sum::<f64>() / window as f64).sqrt())
            .fold(0.0f64, f64::max);
        20.0 * (worst / (0.5 / std::f64::consts::SQRT_2)).log10()
    }
}
