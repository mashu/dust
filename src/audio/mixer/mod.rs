//! The native player's mixer: no devices, no streams, just the arithmetic
//! that turns a band and a send into samples. Kept apart from the cpal glue
//! so every part of it can be tested on a machine with no sound card.
//!
//! Everything the receiver plays is worked out here, a sample at a time and
//! in one place, as one receiver would: the send through the filter, the
//! band, the AGC hearing the two together and riding both, bands handing
//! over to each other, and, without full break-in, its output muted while
//! you send. Your own message in a contest joins after all of it, as a
//! rig's sidetone does, at the volume, and the receiver's AF gain sets how
//! loud the rest is played. The
//! output stage — the paddle's sidetone, and the limiter on everything — is
//! added by the stream, which runs it even when the UI holds the mixer for a
//! moment.

mod band;
mod send;
mod sidetone;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, TryLockError};

use cw_core::band::soft_limit;

use crate::audio::shape::{GATE_RAMP_SEC, Volume};

pub use band::BandVoice;
pub use send::SendVoice;
pub use sidetone::{Sidetone, SidetoneControl};

/// How many bands may be fading out at once. One band hands over to the next
/// in a few tens of milliseconds; a slider dragged across its range rebuilds
/// at most every few, so this many are never all in use.
const FADING_BANDS: usize = 8;

/// How many replaced sends may be riding out their release at once. A
/// release is a few milliseconds; sends are seconds apart.
const RETIRING_SENDS: usize = 4;

/// The band and the Morse, mixed. The UI hands it bands and sends under its
/// lock; the audio callback plays it out.
pub struct Mixer {
    sample_rate: u32,
    band: Option<BandVoice>,
    /// Bands that were replaced, fading out: the newest last.
    fading: Vec<BandVoice>,
    send: Option<SendVoice>,
    /// Sends a newer one replaced, riding out their release.
    retiring: Vec<SendVoice>,
    /// How far behind the last callback the speaker was, in frames: how
    /// long a send started now waits to be heard.
    latency: usize,
    /// How open the receiver's output is: shut while you send without full
    /// break-in, moving between the two a straight line at a time, over
    /// [`GATE_RAMP_SEC`], so it never clicks.
    gate: f64,
    /// What it plays what it receives and what it monitors at, and what it
    /// was last asked to: moving between the two at the gate's pace, a whole
    /// unit of gain over [`GATE_RAMP_SEC`], so a slider moved under a tone
    /// never clicks.
    volume: Volume,
    volume_asked: Volume,
}

/// What the mixer has finished with. A band or a send owns allocations, and
/// freeing them is for the UI thread, never the audio callback — nor while
/// the UI holds the mixer, which the callback is waiting on. So the room for
/// them is made before the lock is taken, and they are dropped after it is
/// let go.
#[must_use = "dropped where the caller is, which should not be the audio thread"]
pub struct Swept {
    bands: Vec<BandVoice>,
    sends: Vec<SendVoice>,
}

impl Swept {
    /// Room for everything one call can sweep, so sweeping never allocates.
    pub fn new() -> Self {
        Self {
            bands: Vec::with_capacity(FADING_BANDS + 2),
            sends: Vec::with_capacity(RETIRING_SENDS + 2),
        }
    }
}

impl Mixer {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: sample_rate.max(1),
            band: None,
            fading: Vec::with_capacity(FADING_BANDS + 1),
            send: None,
            retiring: Vec::with_capacity(RETIRING_SENDS + 1),
            latency: 0,
            gate: 1.0,
            volume: Volume::UNITY,
            volume_asked: Volume::UNITY,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// How long after it starts a send is heard, in seconds, as the last
    /// callback found the speaker behind it.
    pub fn latency_sec(&self) -> f64 {
        self.latency as f64 / f64::from(self.sample_rate)
    }

    /// Play what it receives and what it monitors at `volume` from here on.
    pub fn set_volume(&mut self, volume: Volume) {
        let held = |gain: f64| if gain.is_finite() { gain.max(0.0) } else { 1.0 };
        self.volume_asked = Volume {
            received: held(volume.received),
            monitored: held(volume.monitored),
        };
    }

    /// Put a new band in, or take the band away. The one going out is not
    /// cut off: it fades out while the new one fades in, and the receiver's
    /// gain carries across, so a station being copied rides straight on.
    pub fn set_band(&mut self, band: Option<BandVoice>, swept: &mut Swept) {
        let mut band = band;
        if let (Some(new), Some(old)) = (band.as_mut(), self.band.as_ref()) {
            new.take_over_from(old);
        }
        if let Some(mut old) = std::mem::replace(&mut self.band, band) {
            old.fade_out();
            self.fading.push(old);
        }
        self.sweep(swept);
    }

    /// Start a send at once, as the player does with none asked to start
    /// at a given time. One still sounding is not cut off: it rides out its
    /// release beside the new one.
    #[cfg(test)]
    pub fn start(&mut self, send: SendVoice, swept: &mut Swept) {
        self.start_at(send, None, 0, swept);
    }

    /// Start a send at a given time: `into` samples into the send playing,
    /// which plays on to there, ridden down by then, so the new one takes
    /// over from it sample for sample and the two never sound together, the
    /// receiver muted across the handover where your message runs on into
    /// it; or, with none playing to take over from, or none asked, after
    /// `wait` samples.
    /// Hands back how many samples from the next one played the new send
    /// starts after: none for one asked for where the send playing has
    /// passed, which rides out its release beside the new one.
    pub fn start_at(
        &mut self,
        mut send: SendVoice,
        into: Option<usize>,
        wait: usize,
        swept: &mut Swept,
    ) -> usize {
        let playing = self
            .send
            .as_ref()
            .filter(|playing| playing.is_sounding() && !playing.is_released());
        let wait = match (into, playing) {
            (Some(into), Some(playing)) => playing.samples_until(into),
            _ => wait,
        };
        send.wait(wait);
        if let Some(mut old) = self.send.replace(send) {
            old.release_by(wait);
            self.retiring.push(old);
        }
        self.sweep(swept);
        wait
    }

    /// Stop the send, which rides down over its release, and one a send
    /// waiting to take over from it was to stop later.
    pub fn stop(&mut self) {
        for send in self.send.iter_mut().chain(&mut self.retiring) {
            send.release();
        }
    }

    /// Everything out, for a stream at `sample_rate` to start again with.
    pub fn clear(&mut self, sample_rate: u32, swept: &mut Swept) {
        self.sample_rate = sample_rate.max(1);
        self.gate = 1.0;
        self.volume = self.volume_asked;
        swept.bands.append(&mut self.fading);
        swept.bands.extend(self.band.take());
        swept.sends.append(&mut self.retiring);
        swept.sends.extend(self.send.take());
    }

    /// Take out what has finished playing, and the oldest of what is on its
    /// way out if there is ever more of it than there is room for — by then
    /// it is all but silent.
    pub fn sweep(&mut self, swept: &mut Swept) {
        let mut i = 0;
        while i < self.fading.len() {
            if self.fading[i].is_gone() || self.fading.len() > FADING_BANDS {
                swept.bands.push(self.fading.remove(i));
            } else {
                i += 1;
            }
        }
        let mut i = 0;
        while i < self.retiring.len() {
            if self.retiring[i].is_finished() || self.retiring.len() > RETIRING_SENDS {
                swept.sends.push(self.retiring.remove(i));
            } else {
                i += 1;
            }
        }
    }

    /// One callback's worth of mono frames, over whatever `out` holds. The
    /// speaker is `latency` frames behind: a send is over once its last
    /// sample has been heard, not when it was played.
    pub fn render(&mut self, out: &mut [f32], latency: usize) {
        self.latency = latency;
        // The band's own traffic makes room for a pile-up while it calls.
        let callers = self.send.as_ref().map_or(0, SendVoice::callers_now);
        if let Some(band) = self.band.as_mut() {
            band.reserve_stations(callers);
        }
        let gate_step = 1.0 / (GATE_RAMP_SEC * f64::from(self.sample_rate));
        for slot in out.iter_mut() {
            let mut send = self.send.as_mut().map(SendVoice::next).unwrap_or_default();
            for old in &mut self.retiring {
                send += old.next();
            }
            let mut background = 0.0;
            // The gain the send rides: the band's, or, with none on, the
            // last one's as it goes, easing back to nothing at all to duck
            // behind.
            let mut gain = None;
            if let Some(band) = self.band.as_mut() {
                let played = band.next(send.heard);
                background += played.band;
                gain = Some(played.gain);
            }
            for band in self.fading.iter_mut().rev() {
                let played = band.next(send.heard);
                background += played.band;
                gain.get_or_insert(1.0 + (played.gain - 1.0) * played.weight);
            }
            // Your own message comes in after the AGC: the receiver never
            // hears it, so it neither ducks the reply that follows nor is
            // shaped by the filter, and it is monitored at the volume alone,
            // set at the level it is heard at, not at the AF gain.
            let heard = send.heard * gain.unwrap_or(1.0);
            // Without full break-in the receiver's output, the band and the
            // stations alike, goes quiet while you send, as a rig that is
            // not QSK mutes it; the AGC runs on under it, and your message
            // is never muted.
            let shut = if send.keyed { 0.0 } else { 1.0 };
            self.gate += (shut - self.gate).clamp(-gate_step, gate_step);
            let (asked, now) = (self.volume_asked, &mut self.volume);
            now.received += (asked.received - now.received).clamp(-gate_step, gate_step);
            now.monitored += (asked.monitored - now.monitored).clamp(-gate_step, gate_step);
            *slot +=
                ((background + heard) * self.gate * now.received + send.dry * now.monitored) as f32;
        }
        for send in self.send.iter().chain(&self.retiring) {
            send.settle(latency);
        }
    }
}

/// The stream's side of the mixer: what each callback plays.
pub struct Output {
    mixer: Arc<Mutex<Mixer>>,
    control: Arc<SidetoneControl>,
    sidetone: Sidetone,
    /// The stream's word to the player that something broke.
    failed: Arc<AtomicBool>,
    /// The mixer panicked once; it is not run again on this stream, where it
    /// would only panic on every buffer that followed.
    abandoned: bool,
}

impl Output {
    pub fn new(
        mixer: Arc<Mutex<Mixer>>,
        control: Arc<SidetoneControl>,
        failed: Arc<AtomicBool>,
        sample_rate: u32,
    ) -> Self {
        Self {
            mixer,
            control,
            sidetone: Sidetone::new(sample_rate),
            failed,
            abandoned: false,
        }
    }

    /// One callback's worth of mono frames: the mixer, at its volume, the
    /// paddle's sidetone, set at the level it is heard at, and the limiter
    /// over the lot, `latency` frames ahead of the speaker.
    ///
    /// Never waits. A callback that finds the UI holding the mixer — it only
    /// ever does to swap a band or a send in — goes without the mixer for
    /// that one buffer; the sidetone needs no lock and plays regardless.
    pub fn fill(&mut self, out: &mut [f32], latency: usize) {
        out.fill(0.0);
        if !self.abandoned {
            let guard = match self.mixer.try_lock() {
                Ok(guard) => Some(guard),
                // A panic on the UI side cannot leave the mixer half-written:
                // every change to it is a move into a field.
                Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
                Err(TryLockError::WouldBlock) => None,
            };
            if let Some(mut mixer) = guard
                && catch_unwind(AssertUnwindSafe(|| mixer.render(out, latency))).is_err()
            {
                // A send waiting on this stream fails, and the session
                // rebuilds the player rather than sitting in silence.
                self.abandoned = true;
                self.failed.store(true, Ordering::Release);
                out.fill(0.0);
            }
        }
        self.sidetone.add_to(out, &self.control);
        for sample in out {
            *sample = finish(*sample);
        }
    }
}

/// The output stage: the receiver's soft limiter, once, on everything played
/// together — band, Morse and sidetone — so a crash under a loud pile-up is
/// caught rather than squared off. Anything that is not a number is silence.
pub fn finish(sample: f32) -> f32 {
    if sample.is_finite() {
        (soft_limit(f64::from(sample)) as f32).clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests;
