//! The native player's mixer: no devices, no streams, just the arithmetic
//! that turns a band and a send into samples. Kept apart from the cpal glue
//! so every part of it can be tested on a machine with no sound card.
//!
//! Everything the receiver plays is worked out here, a sample at a time and
//! in one place, as one receiver would: the send through the filter, the
//! band, the AGC hearing the two together and riding both, bands handing
//! over to each other, and, without full break-in, its output muted while
//! you send. The RF gain turns down all the receiver hears before its AGC
//! does, and the AGC hears it all [`AGC_LOOKAHEAD_SEC`] before it is
//! played. Your own message in a contest joins after all of it, as a rig's
//! sidetone does, at the volume, and the receiver's AF gain, each band's
//! its own, sets how loud the rest is played. The
//! output stage — the paddle's sidetone, and the limiter on everything — is
//! added by the stream, which runs it even when the UI holds the mixer for a
//! moment.

mod band;
#[cfg(feature = "native-audio")]
mod output;
mod send;
#[cfg(feature = "native-audio")]
mod sidetone;

#[cfg(doc)]
use cw_core::band::AGC_LOOKAHEAD_SEC;
use cw_core::band::{AudioHighPass, Receiver, agc_lookahead_samples};

use crate::audio::shape::{BAND_FADE_SEC, GATE_RAMP_SEC, Gains};

pub use band::BandVoice;
use band::Glide;
#[cfg(feature = "native-audio")]
pub use output::Output;
#[cfg(all(test, feature = "native-audio"))]
use output::finish;
pub use send::{SendSample, SendVoice, fade_origin_sec};
#[cfg(feature = "native-audio")]
pub use sidetone::{SidetoneControl, clock_sec};

/// How many bands may be fading out at once. One band hands over to the next
/// in a few tens of milliseconds; a slider dragged across its range rebuilds
/// at most every few, so this many are never all in use.
const FADING_BANDS: usize = 8;

/// How many replaced sends may be riding out their release at once. A
/// release is a few milliseconds; sends are seconds apart.
const RETIRING_SENDS: usize = 4;

/// The highest rate the room for the look-ahead is made for up front, so a
/// native stream reopened at another rate needs none made. A stream opened
/// higher has it grown before it starts.
#[cfg(feature = "native-audio")]
const HIGHEST_RATE: u32 = 384_000;

/// The band and the Morse, mixed. The UI hands it bands and sends under its
/// lock; the audio callback plays it out. The browser renders it ahead of
/// its clock, and a copy taken before a stretch plays that stretch again
/// exactly, for it to be made again from any moment in it.
#[derive(Clone)]
pub struct Mixer {
    sample_rate: u32,
    band: Option<BandVoice>,
    /// Bands that were replaced, fading out: the newest last.
    fading: Vec<BandVoice>,
    /// A receiver the band playing is to be retuned to, once it is steady:
    /// one handover at a time, so a slider dragged across its range retunes
    /// to where it has got to each time the last has finished.
    retune: Option<Receiver>,
    send: Option<SendVoice>,
    /// Sends a newer one replaced, riding out their release.
    retiring: Vec<SendVoice>,
    /// How far behind the last callback the speaker was, in frames: how
    /// long a send started now waits to be heard.
    latency: usize,
    /// Whether `latency` was heard from the output now open, rather than
    /// kept from one before it or never heard at all.
    latency_heard: bool,
    /// How open the receiver's output is: shut while you send without full
    /// break-in, moving between the two a straight line at a time, over
    /// [`GATE_RAMP_SEC`], so it never clicks.
    gate: f64,
    /// The gains it plays at, the RF gain ahead of the AGC and the volume
    /// after it, and what it was last asked to: each moving between the two
    /// at the gate's pace, a whole unit of gain over [`GATE_RAMP_SEC`], so a
    /// slider moved under a tone never clicks. The AF gain is each band's
    /// own; this one is the receiver's with no band, which the send rides
    /// alone.
    gains: Gains,
    gains_asked: Gains,
    af: Glide,
    /// The receiver's audio stage, after its AGC: the roll-off below the
    /// note, on the band and the stations, not on your own message.
    audio: AudioHighPass,
    /// What the sends played, waiting out the receiver's look-ahead: its AGC
    /// hears every sample as it comes, through the RF gain as it stands
    /// then, and everything is played that much later at the gain it has by
    /// then, as the band is. Room for the highest rate, made once, of which
    /// the rate in use takes `late_len`.
    late: Vec<SendSample>,
    late_len: usize,
    late_at: usize,
    /// What the send rode on the band going out, last it was on the air,
    /// while the band coming in has still to come in all the way: the send
    /// eases on from there, not from no band at all.
    handed_over: Option<f64>,
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
    receivers: Vec<Receiver>,
}

impl Swept {
    /// Room for everything one call can sweep, so sweeping never allocates.
    pub fn new() -> Self {
        Self {
            bands: Vec::with_capacity(FADING_BANDS + 2),
            sends: Vec::with_capacity(RETIRING_SENDS + 2),
            receivers: Vec::with_capacity(1),
        }
    }
}

impl Mixer {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            audio: AudioHighPass::new(sample_rate),
            sample_rate: sample_rate.max(1),
            band: None,
            fading: Vec::with_capacity(FADING_BANDS + 1),
            retune: None,
            send: None,
            retiring: Vec::with_capacity(RETIRING_SENDS + 1),
            latency: 0,
            latency_heard: false,
            gate: 1.0,
            gains: Gains::UNITY,
            gains_asked: Gains::UNITY,
            af: Glide::at(1.0),
            late: vec![SendSample::default(); late_room(sample_rate)],
            late_len: agc_lookahead_samples(sample_rate),
            late_at: 0,
            handed_over: None,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Whether it has nothing left to play: no band, none still fading out,
    /// and no send sounding or riding out its release. What it plays then is
    /// silence, once the look-ahead has run out. Those gone are counted
    /// gone, swept or not: a change sweeps them, and after the last change
    /// nothing comes to.
    #[cfg(any(feature = "web", test))]
    pub fn is_quiet(&self) -> bool {
        self.band.is_none()
            && self.fading.iter().all(BandVoice::is_gone)
            && self.retiring.iter().all(|send| !send.is_sounding())
            && self.send.as_ref().is_none_or(|send| !send.is_sounding())
    }

    /// Whether a send is still to be heard, or riding out its release: what
    /// a stream moved to another device would cut off.
    #[cfg(feature = "native-audio")]
    pub fn is_sending(&self) -> bool {
        self.send
            .iter()
            .chain(&self.retiring)
            .any(SendVoice::is_sounding)
    }

    /// How long after it starts a send goes to the output, in seconds: the
    /// receiver's look-ahead.
    pub fn lookahead_sec(&self) -> f64 {
        self.late_len as f64 / f64::from(self.sample_rate)
    }

    /// How far behind the output the last callback found the speaker, in
    /// seconds, as the device says.
    #[cfg(feature = "native-audio")]
    pub fn output_latency_sec(&self) -> f64 {
        self.latency as f64 / f64::from(self.sample_rate)
    }

    /// Whether the output now open has said how far behind it the speaker
    /// is: not until its first callback, after a device change.
    #[cfg(feature = "native-audio")]
    pub fn latency_heard(&self) -> bool {
        self.latency_heard
    }

    /// The output is another one now: how far behind it the speaker is
    /// waits to be heard from it.
    #[cfg(feature = "native-audio")]
    pub fn forget_latency(&mut self) {
        self.latency = 0;
        self.latency_heard = false;
    }

    /// Play at `gains` from here on, the band as it is: the AF gain it is
    /// played at glides there over a band's fade, a decibel at a time, or,
    /// with none, the receiver's own.
    pub fn set_gains(&mut self, gains: Gains) {
        self.ask(gains);
        let (af, samples) = (self.gains_asked.af, self.fade_len());
        match self.band.as_mut() {
            Some(band) => band.glide_to(af),
            None => self.af.to(af, samples),
        }
    }

    /// Put a new band in, or take the band away, and play at `gains` from
    /// here on: the band going out keeps the AF gain its receiver was played
    /// at, and the one coming in, or the receiver with none, has the new one
    /// from the start. The send eases from the one to the other with the
    /// fade, so a change of receiver plays neither band at the other's.
    pub fn set_band_at(&mut self, band: Option<BandVoice>, gains: Gains, swept: &mut Swept) {
        self.ask(gains);
        if band.is_none() {
            self.af = Glide::at(self.gains_asked.af);
        }
        self.set_band(band, swept);
    }

    fn ask(&mut self, gains: Gains) {
        let held = |gain: f64| if gain.is_finite() { gain.max(0.0) } else { 1.0 };
        self.gains_asked = Gains {
            rf: held(gains.rf),
            af: held(gains.af),
            volume: held(gains.volume),
        };
    }

    /// How long a band's fade is, in samples.
    fn fade_len(&self) -> usize {
        ((BAND_FADE_SEC * f64::from(self.sample_rate)).round() as usize).max(1)
    }

    /// Retune the band playing to `receiver`, at `gains`, keeping it on the
    /// air: the same noise, static and stations, carried on from where they
    /// are, through the new filter and AGC — as a rig's controls act on what
    /// is on the band, never change who is on it. The new receiver warms in
    /// step with the old, unheard, and the two then cross over a band's
    /// fade. Whether there was a band to retune.
    pub fn retune_band(&mut self, receiver: Receiver, gains: Gains, swept: &mut Swept) -> bool {
        if self.band.is_none() {
            swept.receivers.push(receiver);
            return false;
        }
        self.ask(gains);
        swept.receivers.extend(self.retune.replace(receiver));
        self.sweep(swept);
        true
    }

    /// Start the retune asked for, if the band playing is steady and there
    /// is room for it to go: on the audio thread, at the top of a buffer.
    /// Nothing is allocated: the new band is the old one copied, the room
    /// for the old one made long since.
    fn start_retune(&mut self) {
        let ready = self.band.as_ref().is_some_and(BandVoice::is_steady)
            && self.fading.len() < FADING_BANDS;
        let (Some(current), Some(receiver)) = (self.band.as_mut(), self.retune.take_if(|_| ready))
        else {
            return;
        };
        let mut old = std::mem::replace(current, current.retuned(receiver, self.gains_asked.af));
        old.give_way();
        self.fading.push(old);
    }

    /// Put a new band in, or take the band away. The one going out is not
    /// cut off: it fades out while the new one fades in, and the receiver's
    /// gain carries across, so a station being copied rides straight on. A
    /// retune asked of the old band is for a band that has gone.
    pub fn set_band(&mut self, band: Option<BandVoice>, swept: &mut Swept) {
        swept.receivers.extend(self.retune.take());
        let mut band = band;
        if let Some(new) = band.as_mut() {
            new.start_at(self.gains_asked.af);
            let carried = self
                .band
                .as_ref()
                .is_some_and(|old| new.take_over_from(old));
            if !carried {
                // Nothing to carry over: the AGC hears what is still to be
                // played, oldest first, so the send waiting out the
                // look-ahead is played at the gain it has for it.
                for k in 0..self.late_len {
                    new.hear_ahead(self.late[(self.late_at + k) % self.late_len].heard);
                }
            }
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
    #[cfg(feature = "native-audio")]
    pub fn clear(&mut self, sample_rate: u32, swept: &mut Swept) {
        self.sample_rate = sample_rate.max(1);
        self.forget_latency();
        self.gate = 1.0;
        self.gains = self.gains_asked;
        self.af = Glide::at(self.gains_asked.af);
        self.audio = AudioHighPass::new(self.sample_rate);
        self.late_len = agc_lookahead_samples(self.sample_rate);
        // On the UI thread, before the stream at the new rate starts.
        self.late
            .resize(late_room(self.sample_rate), SendSample::default());
        self.late_at = 0;
        self.handed_over = None;
        swept.bands.append(&mut self.fading);
        swept.bands.extend(self.band.take());
        swept.receivers.extend(self.retune.take());
        self.drop_sends(swept);
    }

    /// Every send out at once, and what of them is still waiting out the
    /// look-ahead: cut, not ridden down, for a stream that starts again, or
    /// one that has skipped on past where they had got to and whose output
    /// was silent there already, where what is left of them would only
    /// play late.
    pub fn drop_sends(&mut self, swept: &mut Swept) {
        swept.sends.append(&mut self.retiring);
        swept.sends.extend(self.send.take());
        self.late.fill(SendSample::default());
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
        self.latency_heard = true;
        self.start_retune();
        // The band's own traffic makes room for a pile-up while it calls.
        let callers = self.send.as_ref().map_or(0, SendVoice::callers_now);
        if let Some(band) = self.band.as_mut() {
            band.reserve_stations(callers);
        }
        let gate_step = 1.0 / (GATE_RAMP_SEC * f64::from(self.sample_rate));
        for slot in out.iter_mut() {
            let (asked, now) = (self.gains_asked, &mut self.gains);
            now.rf += (asked.rf - now.rf).clamp(-gate_step, gate_step);
            now.volume += (asked.volume - now.volume).clamp(-gate_step, gate_step);
            let now = *now;
            // The receiver's AF gain with no band on, which the send rides
            // alone.
            let alone = self.af.next();
            let mut coming = self.send.as_mut().map(SendVoice::next).unwrap_or_default();
            for old in &mut self.retiring {
                coming += old.next();
            }
            // The AGC hears the send as it comes, through the RF gain as it
            // stands now, as it hears the band; the send, your message and
            // the mute are played the look-ahead late, with the band, at the
            // gain the AGC has by then for what it heard.
            let heard_now = SendSample {
                heard: coming.heard * now.rf,
                ..coming
            };
            let send = std::mem::replace(&mut self.late[self.late_at], heard_now);
            self.late_at = (self.late_at + 1) % self.late_len;
            let mut background = 0.0;
            // What the send rides: the AGC's gain at the AF gain of the band
            // riding it.
            // While one band hands over to the next, or to none, the send
            // eases from the one to the other with the fade, the two sized
            // alike for the strongest station, so it never steps.
            let mut current = None;
            if let Some(band) = self.band.as_mut() {
                let played = band.next(coming.heard, now.rf);
                background += played.band;
                current = Some(played);
            }
            // What the send rides going out is the newest band still on the
            // air: one gone has no gain of its own to give, and one replaced
            // as it came in goes before the band it was replacing.
            let mut outgoing = None;
            for band in self.fading.iter_mut().rev() {
                let on_air = !band.is_gone();
                let played = band.next(coming.heard, now.rf);
                background += played.band;
                if on_air {
                    outgoing.get_or_insert(played);
                }
            }
            let riding = match (current, outgoing) {
                (Some(new), old) => {
                    let from = match &old {
                        Some(old) => old.gain * old.af,
                        // Gone before this one came in all the way: from
                        // what it last gave, or with no band before, none.
                        None => self.handed_over.unwrap_or(alone),
                    };
                    if old.is_some() {
                        self.handed_over = Some(from);
                    } else if new.weight >= 1.0 {
                        self.handed_over = None;
                    }
                    let to = new.gain * new.af;
                    from + (to - from) * new.weight
                }
                (None, Some(old)) => {
                    self.handed_over = None;
                    alone + (old.gain * old.af - alone) * old.weight
                }
                (None, None) => {
                    self.handed_over = None;
                    alone
                }
            };
            // Your own message comes in after the AGC: the receiver never
            // hears it, so it neither ducks the reply that follows nor is
            // shaped by the filter, and it is monitored at the volume alone,
            // set at the level it is heard at, not at the RF or the AF gain.
            let heard = send.heard * riding;
            // Without full break-in the receiver's output, the band and the
            // stations alike, goes quiet while you send, as a rig that is
            // not QSK mutes it; the AGC runs on under it, and your message
            // is never muted.
            let shut = if send.keyed { 0.0 } else { 1.0 };
            self.gate += (shut - self.gate).clamp(-gate_step, gate_step);
            // A sample that is not a number would stay in the high-pass for
            // good and silence everything after it.
            let into = background + heard;
            let received = self
                .audio
                .process(if into.is_finite() { into } else { 0.0 });
            let out = received * self.gate + send.dry;
            *slot += (out * now.volume) as f32;
        }
        self.audio.settle();
        // A send is heard the look-ahead after it was played, as well as the
        // speaker's own latency, which is counted from the start of this
        // buffer: by the time the callback returns, only what was played a
        // whole buffer before that has been heard.
        for send in self.send.iter().chain(&self.retiring) {
            send.settle(latency + self.late_len + out.len());
        }
    }
}

/// How much room the send's look-ahead is given at `sample_rate`: natively,
/// enough for the highest rate up front, so a stream reopened at another
/// needs none made, and more for a higher one. The browser's rate never
/// changes, and it copies the mixer for every stretch it renders ahead, so
/// there it is the rate's own.
fn late_room(sample_rate: u32) -> usize {
    #[cfg(feature = "native-audio")]
    let sample_rate = sample_rate.max(HIGHEST_RATE);
    agc_lookahead_samples(sample_rate)
}

#[cfg(all(test, feature = "native-audio"))]
mod tests;
