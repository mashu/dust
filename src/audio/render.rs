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
    let sr = f64::from(sample_rate);
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
    buf
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
            out.fill(0.0);
            self.finished.store(true, Ordering::SeqCst);
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
}

/// The receiver background, as the audio callback plays it out.
pub fn fill_band(out: &mut [f32], channels: usize, mixer: &mut BandMixer, stop: &AtomicBool) {
    if stop.load(Ordering::SeqCst) {
        out.fill(0.0);
        return;
    }
    interleave(out, channels, || mixer.next_background());
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

    #[test]
    fn stopping_silences_the_buffer_at_once() {
        let stop = Arc::new(AtomicBool::new(true));
        let playback = playback(vec![1.0; 64], stop);
        let finished = playback.finished_flag();
        let mut out = [1.0f32; 8];
        playback.fill(&mut out, 1);
        assert_eq!(out, [0.0; 8]);
        assert!(finished.load(Ordering::SeqCst));
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
        // A quarter of a cycle later the same send comes out at a different
        // level; a whole cycle later it is back where it started.
        assert!((level_at(0.0) - level_at(0.5)).abs() > 0.05);
        assert_eq!(level_at(0.0), level_at(2.0));
    }

    #[test]
    fn the_background_fills_until_it_is_stopped() {
        let mut settings = TrainingSettings::default();
        settings.band.qrn_enabled = true;
        settings.band.qrn_level = 1.0;
        settings.band.qrm_enabled = false;
        let mut mixer = BandMixer::new(8_000, &settings, 3);
        let stop = AtomicBool::new(false);
        let mut out = vec![0.0f32; 512];
        fill_band(&mut out, 2, &mut mixer, &stop);
        assert!(out.iter().any(|s| *s != 0.0));
        stop.store(true, Ordering::SeqCst);
        fill_band(&mut out, 2, &mut mixer, &stop);
        assert!(out.iter().all(|s| *s == 0.0));
    }
}
