//! A delay line: what goes in comes out a set number of samples later, as
//! the receiver plays the band, and the player the send, its look-ahead late.

/// A delay of [`Self::len`] samples: each sample pushed in comes back out
/// that many pushes later, silence ([`Default`]) until then. Its room is
/// made once, up front, and may be more than the delay takes, so a delay
/// changed within it ([`Self::reset`]) makes none; nothing else allocates,
/// so the audio thread may run it.
#[derive(Clone)]
pub struct DelayLine<T> {
    /// The room, of which the delay takes the first `len`.
    ring: Vec<T>,
    len: usize,
    /// Where the next sample goes in and the oldest comes out.
    at: usize,
}

impl<T: Copy + Default> DelayLine<T> {
    /// A delay of `len` samples, one at least, with room for no more.
    pub fn new(len: usize) -> Self {
        Self::with_room(len, len)
    }

    /// A delay of `len` samples, one at least, with room for `room`, no
    /// fewer.
    pub fn with_room(room: usize, len: usize) -> Self {
        Self {
            ring: vec![T::default(); room],
            len,
            at: 0,
        }
    }

    /// Put `sample` in, and get back the one put in [`Self::len`] pushes
    /// ago.
    pub fn push(&mut self, sample: T) -> T {
        let due = std::mem::replace(&mut self.ring[self.at], sample);
        self.at = (self.at + 1) % self.len;
        due
    }

    /// How many samples late what goes in comes out. Never none: a delay of
    /// none would have nowhere to hold a sample, so there is no `is_empty`.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.len
    }

    /// What is waiting to come out, the next to first.
    pub fn waiting(&self) -> impl Iterator<Item = T> {
        (0..self.len).map(move |k| self.ring[(self.at + k) % self.len])
    }

    /// Silence for everything waiting, all through the room.
    pub fn clear(&mut self) {
        self.ring.fill(T::default());
    }

    /// Room for `room` from here on, of which a delay of `len`, no more,
    /// takes the first, starting from the top of it. What the room held is
    /// kept where it still fits, and any made is silence ([`Vec::resize`]):
    /// [`Self::clear`] is what silences the rest.
    pub fn reset(&mut self, room: usize, len: usize) {
        self.len = len;
        self.ring.resize(room, T::default());
        self.at = 0;
    }
}
