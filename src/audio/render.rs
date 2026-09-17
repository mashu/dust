//! Sample rendering for the native backend: no devices, no streams, just the
//! arithmetic that turns a [`PlaybackPlan`] into samples. Kept apart from the
//! cpal glue so every part of it can be tested on a machine with no sound card.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use cw_core::band::{qsb_gain_at, BandMixer};
use cw_core::{PlaybackPlan, ToneEvent, TrainingSettings};

/// Fading settings the audio callback can read while the UI changes them.
pub struct LiveQsb {
    enabled: AtomicBool,
    depth_bits: AtomicU64,
    rate_bits: AtomicU64,
}

impl LiveQsb {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            enabled: AtomicBool::new(false),
            depth_bits: AtomicU64::new(0.0f64.to_bits()),
            rate_bits: AtomicU64::new(0.12f64.to_bits()),
        })
    }

    pub fn store(&self, settings: &TrainingSettings) {
        self.enabled
            .store(settings.band.qsb_enabled, Ordering::Relaxed);
        self.depth_bits
            .store(settings.band.qsb_depth.to_bits(), Ordering::Relaxed);
        self.rate_bits
            .store(settings.band.qsb_rate_hz.to_bits(), Ordering::Relaxed);
    }

    pub fn gain_at(&self, t_sec: f64) -> f32 {
        qsb_gain_at(
            t_sec,
            self.enabled.load(Ordering::Relaxed),
            f64::from_bits(self.depth_bits.load(Ordering::Relaxed)),
            f64::from_bits(self.rate_bits.load(Ordering::Relaxed)),
        )
    }
}

/// How long a stopped send takes to reach silence.
///
/// Matched to the keying envelope's own rise time: a trainer this careful
/// about click-free keying should not end a session with a click. Short enough
/// that "stop" still means stop.
pub const RELEASE_MS: f64 = 8.0;

/// The release shape — a raised cosine, the same family the keying envelope
/// uses, so a cut send tails off like the end of a dit rather than a fade-out.
pub fn release_gain(progress: f64) -> f32 {
    let p = progress.clamp(0.0, 1.0);
    (0.5 * (1.0 + (std::f64::consts::PI * p).cos())) as f32
}

fn release_samples(sample_rate: f64) -> usize {
    ((RELEASE_MS / 1000.0) * sample_rate).round().max(1.0) as usize
}

pub fn envelope_gain(event: &ToneEvent, t: f64) -> f32 {
    let curve = &event.envelope;
    if curve.len() < 2 || event.duration_sec <= 0.0 {
        return event.target_gain as f32;
    }
    let rel = (t / event.duration_sec).clamp(0.0, 1.0);
    let pos = rel * (curve.len() - 1) as f64;
    let i = pos.floor() as usize;
    let frac = (pos - i as f64) as f32;
    let a = curve.get(i).copied().unwrap_or(0.0);
    let b = curve.get(i + 1).copied().unwrap_or(a);
    a * (1.0 - frac) + b * frac
}

/// Render a plan to mono samples, with a little tail so the last symbol is not
/// cut off by a buffer boundary.
pub fn render_plan(plan: &PlaybackPlan, sample_rate: u32) -> Vec<f32> {
    let sample_rate = sample_rate.max(1);
    let extra = sample_rate / 20;
    let n = ((plan.duration_sec.max(0.0) * f64::from(sample_rate)).ceil() as usize)
        .saturating_add(extra as usize);
    let mut buf = vec![0.0f32; n.max(1)];
    mix_plan_into(&mut buf, plan, sample_rate);
    buf
}

/// Add one station's sound to a buffer that may already hold others.
///
/// Stations add — that is all interference is. Each one carries its own pitch
/// and its own level in its events, so a pile-up is this called once per
/// station over the same buffer.
pub fn mix_plan_into(buf: &mut [f32], plan: &PlaybackPlan, sample_rate: u32) {
    let sr = f64::from(sample_rate.max(1));
    for event in &plan.events {
        let start = (event.start_sec * sr).round().max(0.0) as usize;
        let len = ((event.duration_sec * sr).round().max(0.0) as usize).max(1);
        let two_pi_f = 2.0 * std::f64::consts::PI * event.frequency_hz;
        for i in 0..len {
            let t = i as f64 / sr;
            let sample = (two_pi_f * t).sin() as f32 * envelope_gain(event, t);
            if let Some(slot) = buf.get_mut(start + i) {
                *slot += sample;
            }
        }
    }
}

/// Spread one mono sample across every channel of an interleaved buffer.
pub fn interleave(out: &mut [f32], channels: usize, mut next: impl FnMut() -> f32) {
    let channels = channels.max(1);
    let mut i = 0;
    while i < out.len() {
        let value = next();
        for _ in 0..channels {
            if i >= out.len() {
                break;
            }
            out[i] = value;
            i += 1;
        }
    }
}

/// One send, as the audio callback plays it out.
pub struct TonePlayback {
    samples: Vec<f32>,
    sample_rate: f64,
    /// Where this send sits on the band's own clock. Fading is a property of
    /// the path, not of the group: starting every send at the same point of
    /// the QSB cycle would give every group an identical fade.
    started_at_sec: f64,
    pos: AtomicUsize,
    /// How far into the release ramp a stopped send has got.
    released: AtomicUsize,
    finished: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    qsb: Arc<LiveQsb>,
}

impl TonePlayback {
    pub fn new(
        samples: Vec<f32>,
        sample_rate: u32,
        started_at_sec: f64,
        qsb: Arc<LiveQsb>,
        stop: Arc<AtomicBool>,
    ) -> Self {
        Self {
            samples,
            sample_rate: f64::from(sample_rate.max(1)),
            started_at_sec,
            pos: AtomicUsize::new(0),
            released: AtomicUsize::new(0),
            finished: Arc::new(AtomicBool::new(false)),
            stop,
            qsb,
        }
    }

    pub fn finished_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.finished)
    }

    /// Fill one callback's worth of interleaved samples.
    pub fn fill(&self, out: &mut [f32], channels: usize) {
        if self.stop.load(Ordering::SeqCst) {
            self.fill_release(out, channels);
            return;
        }
        let mut i = self.pos.load(Ordering::SeqCst);
        let sr = self.sample_rate;
        interleave(out, channels, || {
            let Some(dry) = self.samples.get(i).copied() else {
                return 0.0;
            };
            let value = dry * self.qsb.gain_at(self.started_at_sec + i as f64 / sr);
            i += 1;
            value
        });
        self.pos.store(i, Ordering::SeqCst);
        if i >= self.samples.len() {
            self.finished.store(true, Ordering::SeqCst);
        }
    }

    /// A stopped send keeps playing for a few milliseconds, under a falling
    /// ramp. Cutting the samples to zero where they happen to be is a step in
    /// the waveform, and a step is a click.
    fn fill_release(&self, out: &mut [f32], channels: usize) {
        let total = release_samples(self.sample_rate);
        let mut done = self.released.load(Ordering::SeqCst);
        let mut i = self.pos.load(Ordering::SeqCst);
        let sr = self.sample_rate;
        interleave(out, channels, || {
            if done >= total {
                return 0.0;
            }
            let dry = self.samples.get(i).copied().unwrap_or(0.0);
            let value = dry
                * self.qsb.gain_at(self.started_at_sec + i as f64 / sr)
                * release_gain(done as f64 / total as f64);
            i += 1;
            done += 1;
            value
        });
        self.pos.store(i, Ordering::SeqCst);
        self.released.store(done, Ordering::SeqCst);
        if done >= total {
            self.finished.store(true, Ordering::SeqCst);
        }
    }
}

/// The receiver background, as the audio callback plays it out.
pub struct BandPlayback {
    mixer: BandMixer,
    stop: Arc<AtomicBool>,
    released: usize,
    sample_rate: f64,
}

impl BandPlayback {
    pub fn new(mixer: BandMixer, stop: Arc<AtomicBool>, sample_rate: u32) -> Self {
        Self {
            mixer,
            stop,
            released: 0,
            sample_rate: f64::from(sample_rate.max(1)),
        }
    }

    /// Fill one callback's worth, fading out once the background is stopped
    /// rather than dropping to silence mid-sample.
    pub fn fill(&mut self, out: &mut [f32], channels: usize) {
        if !self.stop.load(Ordering::SeqCst) {
            let mixer = &mut self.mixer;
            interleave(out, channels, || mixer.next_background());
            return;
        }
        let total = release_samples(self.sample_rate);
        let mut done = self.released;
        let mixer = &mut self.mixer;
        interleave(out, channels, || {
            if done >= total {
                return 0.0;
            }
            let value = mixer.next_background() * release_gain(done as f64 / total as f64);
            done += 1;
            value
        });
        self.released = done;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cw_core::{plan_morse_playback, FastrandRng};

    fn plan(text: &str) -> PlaybackPlan {
        let mut settings = TrainingSettings::default();
        settings.playback.char_wpm_min = 20.0;
        settings.playback.char_wpm_max = 20.0;
        settings.playback.effective_wpm_min = 20.0;
        settings.playback.effective_wpm_max = 20.0;
        settings.band.link_volume = true;
        settings.band.volume_min = 1.0;
        let mut rng = FastrandRng(1);
        plan_morse_playback(text, &settings, &mut rng)
    }

    #[test]
    fn a_rendered_send_is_silent_at_both_ends_and_loud_in_the_middle() {
        let plan = plan("K");
        let samples = render_plan(&plan, 8_000);
        assert!(samples.len() > (plan.duration_sec * 8_000.0) as usize);
        assert_eq!(samples.first().copied(), Some(0.0));
        assert_eq!(samples.last().copied(), Some(0.0));
        let peak = samples.iter().fold(0.0f32, |acc, s| acc.max(s.abs()));
        assert!(peak > 0.1, "peak was {peak}");
        assert!(peak <= 1.0);
    }

    #[test]
    fn a_send_with_nothing_in_it_still_renders_a_buffer() {
        let plan = plan("");
        let samples = render_plan(&plan, 8_000);
        assert!(!samples.is_empty());
        assert!(samples.iter().all(|s| *s == 0.0));
        // A zero sample rate is treated as one rather than dividing by it.
        assert!(!render_plan(&plan, 0).is_empty());
    }

    #[test]
    fn the_envelope_is_read_off_the_curve() {
        let event = ToneEvent {
            start_sec: 0.0,
            duration_sec: 1.0,
            frequency_hz: 600.0,
            target_gain: 0.5,
            envelope: vec![0.0, 1.0],
        };
        assert_eq!(envelope_gain(&event, 0.0), 0.0);
        assert_eq!(envelope_gain(&event, 0.5), 0.5);
        assert_eq!(envelope_gain(&event, 1.0), 1.0);
        // Outside the symbol the curve is clamped, not extrapolated.
        assert_eq!(envelope_gain(&event, -1.0), 0.0);
        assert_eq!(envelope_gain(&event, 9.0), 1.0);
    }

    #[test]
    fn a_symbol_with_no_curve_holds_its_level() {
        let flat = ToneEvent {
            start_sec: 0.0,
            duration_sec: 1.0,
            frequency_hz: 600.0,
            target_gain: 0.25,
            envelope: vec![1.0],
        };
        assert_eq!(envelope_gain(&flat, 0.5), 0.25);
        let instant = ToneEvent {
            duration_sec: 0.0,
            ..flat
        };
        assert_eq!(envelope_gain(&instant, 0.0), 0.25);
    }

    #[test]
    fn every_channel_gets_the_same_sample() {
        let mut out = [0.0f32; 6];
        let mut n = 0.0;
        interleave(&mut out, 2, || {
            n += 1.0;
            n
        });
        assert_eq!(out, [1.0, 1.0, 2.0, 2.0, 3.0, 3.0]);

        // A device claiming no channels is treated as mono.
        let mut out = [0.0f32; 3];
        let mut n = 0.0;
        interleave(&mut out, 0, || {
            n += 1.0;
            n
        });
        assert_eq!(out, [1.0, 2.0, 3.0]);

        // A buffer that ends mid-frame is not written past.
        let mut out = [0.0f32; 3];
        interleave(&mut out, 2, || 1.0);
        assert_eq!(out, [1.0, 1.0, 1.0]);
        interleave(&mut [], 2, || panic!("nothing to fill"));
    }

    fn playback(samples: Vec<f32>, stop: Arc<AtomicBool>) -> TonePlayback {
        let qsb = LiveQsb::new();
        let mut settings = TrainingSettings::default();
        settings.band.qsb_enabled = false;
        qsb.store(&settings);
        TonePlayback::new(samples, 8_000, 0.0, qsb, stop)
    }

    #[test]
    fn a_send_plays_out_and_then_reports_it_is_finished() {
        let stop = Arc::new(AtomicBool::new(false));
        let playback = playback(vec![0.5; 4], Arc::clone(&stop));
        let finished = playback.finished_flag();
        let mut out = [0.0f32; 4];
        playback.fill(&mut out, 2);
        // Two frames of a stereo buffer is two samples of the send.
        assert_eq!(out, [0.5, 0.5, 0.5, 0.5]);
        assert!(!finished.load(Ordering::SeqCst));
        playback.fill(&mut out, 2);
        assert!(finished.load(Ordering::SeqCst));
        // Past the end it keeps quiet instead of repeating.
        let mut tail = [1.0f32; 4];
        playback.fill(&mut tail, 2);
        assert_eq!(tail, [0.0; 4]);
    }

    /// A send stopped mid-tone is ramped down, not cut. Dropping the samples
    /// to zero where they happen to be is a step in the waveform, and a step
    /// is a click — which is a strange way for a trainer that shapes every dit
    /// to end a session.
    #[test]
    fn stopping_rides_the_send_down_instead_of_cutting_it() {
        let release = release_samples(8_000.0);
        let stop = Arc::new(AtomicBool::new(false));
        let playback = playback(vec![1.0; release * 4], Arc::clone(&stop));
        let finished = playback.finished_flag();

        let mut out = vec![0.0f32; 16];
        playback.fill(&mut out, 1);
        assert!(out.iter().all(|s| *s == 1.0), "it should be sounding");

        stop.store(true, Ordering::SeqCst);
        let mut tail = vec![0.0f32; release + 16];
        playback.fill(&mut tail, 1);

        // It starts where the tone was, not at zero...
        assert!(tail[0] > 0.99, "the ramp jumped: first sample {}", tail[0]);
        // ...comes down without ever rising...
        assert!(
            tail.windows(2).all(|w| w[1] <= w[0] + 1e-6),
            "the release should only ever fall"
        );
        // ...and gets all the way to silence within the release time.
        assert_eq!(tail[release..], vec![0.0; tail.len() - release][..]);
        assert!(finished.load(Ordering::SeqCst));

        // No step anywhere in it: every neighbouring pair is a small change.
        let biggest = tail
            .windows(2)
            .map(|w| (w[0] - w[1]).abs())
            .fold(0.0f32, f32::max);
        assert!(biggest < 0.05, "the release still had a step of {biggest}");
    }

    #[test]
    fn the_release_shape_runs_from_full_to_silent() {
        assert_eq!(release_gain(0.0), 1.0);
        assert_eq!(release_gain(1.0), 0.0);
        assert!(release_gain(0.5) > 0.4 && release_gain(0.5) < 0.6);
        assert_eq!(release_gain(-1.0), 1.0, "out of range is still in range");
        assert_eq!(release_gain(2.0), 0.0);
        let mut previous = 2.0;
        for step in 0..=20 {
            let value = release_gain(f64::from(step) / 20.0);
            assert!(value < previous, "the release rose at {step}");
            previous = value;
        }
    }

    #[test]
    fn fading_is_applied_to_the_samples_as_they_go_out() {
        let qsb = LiveQsb::new();
        let mut settings = TrainingSettings::default();
        settings.band.qsb_enabled = true;
        settings.band.qsb_depth = 0.75;
        settings.band.qsb_rate_hz = 1.5;
        qsb.store(&settings);
        let playback = TonePlayback::new(
            vec![1.0; 8_000],
            8_000,
            0.0,
            qsb,
            Arc::new(AtomicBool::new(false)),
        );
        let mut out = vec![0.0f32; 8_000];
        playback.fill(&mut out, 1);
        let min = out.iter().copied().fold(f32::MAX, f32::min);
        let max = out.iter().copied().fold(f32::MIN, f32::max);
        assert!(max - min > 0.1, "fading did not move the level");
        assert!(out.iter().all(|s| *s >= 0.0 && *s <= 1.0));
    }

    #[test]
    fn fading_carries_on_where_the_last_send_left_off() {
        let qsb = LiveQsb::new();
        let mut settings = TrainingSettings::default();
        settings.band.qsb_enabled = true;
        settings.band.qsb_depth = 0.75;
        settings.band.qsb_rate_hz = 0.5;
        qsb.store(&settings);
        let level_at = |offset: f64| {
            let playback = TonePlayback::new(
                vec![1.0; 8],
                8_000,
                offset,
                Arc::clone(&qsb),
                Arc::new(AtomicBool::new(false)),
            );
            let mut out = [0.0f32; 8];
            playback.fill(&mut out, 1);
            out[0]
        };
        // The same send played at a different point on the band's clock comes
        // out at a different level.
        assert!((level_at(0.0) - level_at(0.5)).abs() > 0.05);

        // And that level is the band's, not the send's: what a send hears is
        // whatever the fading is doing at the moment it starts, so a send is
        // dropped into a fade already in progress rather than starting one.
        //
        // This used to be checked by playing a whole cycle later and expecting
        // the same level back. That only held while fading was a single sine,
        // and a fade you can predict a cycle ahead is the one thing real QSB
        // never is — so it asks the shared clock directly now.
        for offset in [0.0, 0.37, 1.4, 6.25, 41.0] {
            let expected = cw_core::band::qsb_gain_at(offset, true, 0.75, 0.5);
            assert!(
                (level_at(offset) - expected).abs() < 1e-6,
                "a send at {offset}s played at {}, but the band was at {expected}",
                level_at(offset)
            );
        }
    }

    #[test]
    fn the_background_fills_until_it_is_stopped_and_then_fades() {
        let mut settings = TrainingSettings::default();
        settings.band.qrn_enabled = true;
        settings.band.qrn_level = 1.0;
        settings.band.receiver_enabled = false;
        let mixer = BandMixer::new(8_000, &settings, 3);
        let stop = Arc::new(AtomicBool::new(false));
        let mut playback = BandPlayback::new(mixer, Arc::clone(&stop), 8_000);
        let mut out = vec![0.0f32; 512];
        playback.fill(&mut out, 2);
        assert!(out.iter().any(|s| *s != 0.0));

        stop.store(true, Ordering::SeqCst);
        // The ramp is 8 ms, so at 8 kHz it outlives one 512-frame callback.
        playback.fill(&mut out, 2);
        assert!(out.iter().any(|s| *s != 0.0), "it cut instead of fading");
        for _ in 0..8 {
            playback.fill(&mut out, 2);
        }
        assert!(out.iter().all(|s| *s == 0.0), "the fade never finished");
    }
}
