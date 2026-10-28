//! The receiver's level meter: what the audio callback measured, a short
//! stretch at a time, kept where the UI reads it without taking the mixer's
//! lock.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use crate::audio::{METER_FRAME_SEC, MeterFrame};

/// How many frames are kept: five seconds of them, far longer than the
/// scope waits between looks, so a reader that falls behind for a moment
/// loses nothing; the scope keeps its own history.
const KEPT: usize = 512;

/// The last [`KEPT`] frames, written by the audio callback and read by the
/// UI. Atomics, so neither ever waits on the other; a frame read while it
/// is overwritten is only ever one five seconds old being replaced.
pub struct Meter {
    input: [AtomicU32; KEPT],
    held: [AtomicU32; KEPT],
    output: [AtomicU32; KEPT],
    /// How many frames have ever been written.
    written: AtomicUsize,
}

impl Meter {
    /// How many of its last frames it keeps for a reader ([`KEPT`]).
    #[cfg(any(feature = "web", test))]
    pub const KEPT: usize = KEPT;

    pub fn new() -> Self {
        let zeros = || std::array::from_fn(|_| AtomicU32::new(0));
        Self {
            input: zeros(),
            held: zeros(),
            output: zeros(),
            written: AtomicUsize::new(0),
        }
    }

    fn write(&self, frame: MeterFrame) {
        let n = self.written.load(Ordering::Relaxed);
        let at = n % KEPT;
        self.input[at].store(frame.input.to_bits(), Ordering::Relaxed);
        self.held[at].store(frame.held.to_bits(), Ordering::Relaxed);
        self.output[at].store(frame.output.to_bits(), Ordering::Relaxed);
        self.written.store(n + 1, Ordering::Release);
    }

    /// The frames written since `seen` frames had been, oldest first, onto
    /// `into`, and `seen` moved on past them. Of a reader that fell more
    /// than [`KEPT`] behind, only the newest are left to read.
    pub fn read_since(&self, seen: &mut usize, into: &mut Vec<MeterFrame>) {
        let written = self.written.load(Ordering::Acquire);
        let from = (*seen).max(written.saturating_sub(KEPT));
        for n in from..written {
            let at = n % KEPT;
            let read = |slot: &[AtomicU32; KEPT]| f32::from_bits(slot[at].load(Ordering::Relaxed));
            into.push(MeterFrame {
                input: read(&self.input),
                held: read(&self.held),
                output: read(&self.output),
            });
        }
        *seen = written;
    }
}

/// The frame the callback is filling: its peaks so far, and how many samples
/// it still has to take. A copy writes to the same meter.
#[derive(Clone)]
pub struct Metering {
    meter: Arc<Meter>,
    frame_len: usize,
    left: usize,
    frame: MeterFrame,
}

impl Metering {
    pub fn new(meter: Arc<Meter>, sample_rate: u32) -> Self {
        let frame_len = ((METER_FRAME_SEC * f64::from(sample_rate.max(1))) as usize).max(1);
        Self {
            meter,
            frame_len,
            left: frame_len,
            frame: MeterFrame::QUIET,
        }
    }

    pub fn meter(&self) -> Arc<Meter> {
        Arc::clone(&self.meter)
    }

    /// How many samples of the frame in hand have been taken.
    #[cfg(any(feature = "web", test))]
    pub fn taken(&self) -> usize {
        self.frame_len - self.left
    }

    /// Start again at `sample_rate`, the frame in hand dropped.
    #[cfg(feature = "native-audio")]
    pub fn restart(&mut self, sample_rate: u32) {
        *self = Self::new(Arc::clone(&self.meter), sample_rate);
    }

    /// One sample: what reached the AGC, the gain its hang held, and what
    /// the receiver put out, before the volume.
    pub fn add(&mut self, input: f64, held: f64, output: f64) {
        self.frame = self.frame.merge(MeterFrame {
            input: input.abs() as f32,
            held: held as f32,
            output: output.abs() as f32,
        });
        self.left -= 1;
        if self.left == 0 {
            self.meter.write(self.frame);
            self.frame = MeterFrame::QUIET;
            self.left = self.frame_len;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frames come out in the order they went in, each once, and a reader
    /// that fell far behind gets the newest it can, not a wrapped mixture.
    #[test]
    fn frames_are_read_once_in_order_and_the_newest_survive_a_slow_reader() {
        let meter = Meter::new();
        let mut seen = 0;
        let mut read = Vec::new();
        for i in 0..3 {
            meter.write(MeterFrame {
                input: i as f32,
                held: 1.0,
                output: 0.0,
            });
        }
        meter.read_since(&mut seen, &mut read);
        assert_eq!(
            read.iter().map(|f| f.input).collect::<Vec<_>>(),
            [0.0, 1.0, 2.0]
        );
        read.clear();
        meter.read_since(&mut seen, &mut read);
        assert!(read.is_empty(), "read twice");

        for i in 0..(KEPT + 10) {
            meter.write(MeterFrame {
                input: i as f32,
                held: 1.0,
                output: 0.0,
            });
        }
        meter.read_since(&mut seen, &mut read);
        assert_eq!(read.len(), KEPT);
        assert_eq!(read.first().map(|f| f.input), Some(10.0));
        assert_eq!(read.last().map(|f| f.input), Some((KEPT + 9) as f32));
    }

    /// A frame is a stretch's peaks and its lowest held gain, sign aside.
    #[test]
    fn a_frame_holds_the_peaks_and_the_lowest_held_gain_of_its_stretch() {
        let meter = Arc::new(Meter::new());
        let mut metering = Metering::new(Arc::clone(&meter), 400);
        // Four samples a frame at 400 Hz.
        for (input, held, output) in [(0.1, 1.0, 0.2), (-0.5, 0.6, -0.9), (0.2, 0.8, 0.1)] {
            metering.add(input, held, output);
        }
        let (mut seen, mut read) = (0, Vec::new());
        meter.read_since(&mut seen, &mut read);
        assert!(read.is_empty(), "the frame is not over");
        metering.add(0.0, 1.0, 0.0);
        meter.read_since(&mut seen, &mut read);
        assert_eq!(
            read,
            [MeterFrame {
                input: 0.5,
                held: 0.6,
                output: 0.9
            }]
        );
    }
}
