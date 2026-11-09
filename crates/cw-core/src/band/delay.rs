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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::{Rng, SplitMix64};

    /// The `i`th sample pushed: each its own, and none of them silence.
    fn sample(i: usize) -> u64 {
        i as u64 + 1
    }

    /// The whole room, slot by slot from the top: what a delay of all of it
    /// plays out next, as a reset to one keeps it.
    fn whole_room<T: Copy + Default>(line: &DelayLine<T>, room: usize) -> Vec<T> {
        let mut whole = line.clone();
        whole.reset(room, room);
        whole.waiting().collect()
    }

    /// Each sample comes back out `len` pushes after it went in, and until
    /// the first has, what comes out is silence.
    #[test]
    fn a_sample_comes_out_len_pushes_later() {
        for len in [1, 2, 3, 4, 5, 8, 63, 64, 1_000, 4_096] {
            let mut line = DelayLine::new(len);
            assert_eq!(line.len(), len);
            for i in 0..3 * len + 2 {
                let due = i.checked_sub(len).map_or(0, sample);
                assert_eq!(line.push(sample(i)), due, "a delay of {len}, push {i}");
            }
            assert_eq!(line.len(), len);
        }
    }

    /// Room beyond the delay changes nothing it plays, and no push touches
    /// it: it is silence for a reset to a longer delay to bring in.
    #[test]
    fn room_beyond_the_delay_plays_no_part_and_no_push_touches_it() {
        for (room, len) in [
            (1, 1),
            (2, 1),
            (3, 2),
            (8, 3),
            (64, 63),
            (3_840, 1),
            (3_840, 480),
        ] {
            let mut roomy = DelayLine::with_room(room, len);
            let mut tight = DelayLine::new(len);
            assert_eq!(roomy.len(), len);
            for i in 0..3 * len + 2 {
                assert_eq!(
                    roomy.push(sample(i)),
                    tight.push(sample(i)),
                    "room for {room}, a delay of {len}, push {i}"
                );
            }
            // The pushes have filled every slot of the delay.
            let whole = whole_room(&roomy, room);
            assert!(
                whole[..len].iter().all(|&s| s != 0),
                "room for {room}, a delay of {len}: the room is seen to hold what was pushed"
            );
            assert_eq!(
                whole[len..],
                vec![0; room - len],
                "room for {room}, a delay of {len}"
            );
        }
    }

    /// What is waiting is the next `len` to come out, the next first: the
    /// last `len` pushed, oldest first, behind silence for any not yet
    /// pushed; just what the next `len` pushes give back.
    #[test]
    fn waiting_is_the_next_len_to_come_out_the_next_first() {
        for (room, len) in [(1, 1), (2, 2), (3, 2), (5, 5), (9, 4), (64, 17), (100, 99)] {
            let mut line = DelayLine::with_room(room, len);
            for pushed in 0..3 * len + 2 {
                let here = format!("room for {room}, a delay of {len}, after {pushed}");
                let waiting: Vec<u64> = line.waiting().collect();
                let due: Vec<u64> = (pushed..pushed + len)
                    .map(|i| i.checked_sub(len).map_or(0, sample))
                    .collect();
                assert_eq!(waiting, due, "{here}");
                let mut ahead = line.clone();
                let given_back: Vec<u64> = (0..len).map(|_| ahead.push(0)).collect();
                assert_eq!(waiting, given_back, "{here}");
                line.push(sample(pushed));
            }
        }
    }

    /// A clear is silence for everything waiting, all through the room: the
    /// part beyond the delay too, which a reset to a longer one would bring
    /// back into play. Silence comes out until the first sample pushed since
    /// is due.
    #[test]
    fn a_clear_silences_everything_waiting_all_through_the_room() {
        for (room, len) in [(1, 1), (2, 1), (8, 3), (9, 9), (100, 37), (3_840, 1_920)] {
            let here = format!("room for {room}, a delay of {len}");
            // The whole room played through, then the delay shortened and
            // pushed part of the way round: the room beyond it holds what a
            // longer delay would play.
            let mut line = DelayLine::new(room);
            for i in 0..room + room / 2 {
                line.push(sample(i));
            }
            line.reset(room, len);
            for i in 0..len / 2 + 1 {
                line.push(sample(room + i));
            }
            assert!(whole_room(&line, room).iter().all(|&s| s != 0), "{here}");
            line.clear();
            assert_eq!(line.len(), len, "{here}");
            assert_eq!(line.waiting().collect::<Vec<_>>(), vec![0; len], "{here}");
            assert_eq!(whole_room(&line, room), vec![0; room], "{here}");
            for i in 0..2 * len {
                let due = i.checked_sub(len).map_or(0, sample);
                assert_eq!(line.push(sample(i)), due, "{here}, push {i}");
            }
        }
    }

    /// A reset keeps what the room held where it still fits, slot for slot,
    /// beyond the new delay too; what it grows the room by is silence, and
    /// so is what a room it cut grows back.
    #[test]
    fn a_reset_keeps_the_room_where_it_still_fits_and_grows_it_with_silence() {
        // A room for `room` with the first `up_to` samples in their slots,
        // the ith in slot i, and silence after them.
        let held = |up_to: usize, room: usize| -> Vec<u64> {
            (0..room)
                .map(|i| if i < up_to { sample(i) } else { 0 })
                .collect()
        };
        for room in [1, 2, 3, 8, 50] {
            for new_room in [1_usize, 2, 3, 7, 8, 9, 50, 64] {
                for new_len in [1, new_room.div_ceil(2), new_room] {
                    let here =
                        format!("room for {room}, then for {new_room}, a delay of {new_len}");
                    // Filled from the top, where a reset starts a line: where
                    // one just made starts, its docs leave open.
                    let mut line = DelayLine::new(room);
                    line.reset(room, room);
                    for i in 0..room {
                        line.push(sample(i));
                    }
                    line.reset(new_room, new_len);
                    let kept = held(room, new_room);
                    assert_eq!(line.len(), new_len, "{here}");
                    assert_eq!(
                        line.waiting().collect::<Vec<_>>(),
                        kept[..new_len],
                        "{here}"
                    );
                    assert_eq!(whole_room(&line, new_room), kept, "{here}");
                    assert_eq!(
                        whole_room(&line, room),
                        held(room.min(new_room), room),
                        "{here}, and back"
                    );
                }
            }
        }
    }

    /// After a reset, wherever the line had got to, the next sample goes in
    /// at the top of the room: what comes out first is what the room held
    /// from the top, slot by slot, and what is pushed from then on comes out
    /// in turn once the new delay has passed.
    #[test]
    fn after_a_reset_the_next_sample_goes_in_at_the_top() {
        for (room, len) in [(1, 1), (2, 2), (3, 2), (5, 5), (9, 4)] {
            for pushed in 0..2 * len + 1 {
                for new_len in 1..=room {
                    let here = format!(
                        "room for {room}, a delay of {len}, reset after {pushed} to {new_len}"
                    );
                    // The room as the pushes left it, from the top, where the
                    // first reset starts them: the ith in slot i % len.
                    let mut line = DelayLine::with_room(room, len);
                    line.reset(room, len);
                    let mut held = vec![0; room];
                    for i in 0..pushed {
                        line.push(sample(i));
                        held[i % len] = sample(i);
                    }
                    line.reset(room, new_len);
                    let first: Vec<u64> = (0..new_len)
                        .map(|i| line.push(sample(pushed + i)))
                        .collect();
                    assert_eq!(first, held[..new_len], "{here}");
                    let then: Vec<u64> = (0..new_len).map(|_| line.push(0)).collect();
                    let since: Vec<u64> = (pushed..pushed + new_len).map(sample).collect();
                    assert_eq!(then, since, "{here}");
                }
            }
        }
    }

    /// A reset and then a clear, as the mixer does for a stream that starts
    /// again, leave a line just as one made for the new delay and room:
    /// the new delay's worth of silence waiting, silence all through the
    /// room, and silence out until the first sample pushed since is due.
    #[test]
    fn a_reset_then_a_clear_is_a_line_just_made() {
        let lines = [(1, 1), (8, 3), (64, 64), (3_840, 480), (3_840, 441)];
        // Among them the mixer's own, in the room it makes for the highest
        // rate: a stream started again at 48 or 44.1 kHz, after one at either.
        let resets = [
            (1, 1),
            (8, 1),
            (8, 8),
            (64, 17),
            (3_840, 441),
            (3_840, 480),
            (3_840, 960),
            (3_840, 3_840),
        ];
        for (room, len) in lines {
            for (new_room, new_len) in resets {
                let here =
                    format!("room for {room}, a delay of {len}, reset to {new_room}/{new_len}");
                let mut line = DelayLine::with_room(room, len);
                for i in 0..len + len / 2 + 1 {
                    line.push(sample(i));
                }
                line.reset(new_room, new_len);
                assert!(
                    whole_room(&line, new_room)[..len.min(new_room)]
                        .iter()
                        .all(|&s| s != 0),
                    "{here}: the room is seen to hold what was pushed"
                );
                line.clear();
                let mut made = DelayLine::with_room(new_room, new_len);
                assert_eq!(line.len(), made.len(), "{here}");
                assert_eq!(
                    line.waiting().collect::<Vec<_>>(),
                    vec![0; new_len],
                    "{here}"
                );
                assert_eq!(
                    whole_room(&line, new_room),
                    whole_room(&made, new_room),
                    "{here}"
                );
                for i in 0..2 * new_len + 1 {
                    let pushed = sample(i);
                    assert_eq!(line.push(pushed), made.push(pushed), "{here}, push {i}");
                }
            }
        }
    }

    /// The room is made once, up front, and is what it was made with: a
    /// delay changed within it, longer or shorter, cleared or not, keeps
    /// that room rather than making another.
    #[test]
    fn a_delay_changed_within_its_room_makes_no_room() {
        for (room, len) in [(1, 1), (8, 1), (8, 8), (3_840, 480)] {
            for mut line in [DelayLine::<f64>::with_room(room, len), DelayLine::new(room)] {
                assert_eq!(line.ring.len(), room);
                let (start, capacity) = (line.ring.as_ptr(), line.ring.capacity());
                for new_len in [1, room.div_ceil(2), room, len] {
                    let here = format!("room for {room}, a delay of {len}, then {new_len}");
                    for i in 0..room + 1 {
                        line.push(i as f64);
                    }
                    line.reset(room, new_len);
                    assert_eq!(line.ring.as_ptr(), start, "{here}");
                    assert_eq!(line.ring.capacity(), capacity, "{here}");
                    line.clear();
                    assert_eq!(line.ring.as_ptr(), start, "{here}");
                    assert_eq!(line.ring.capacity(), capacity, "{here}");
                }
            }
        }
    }

    /// Samples come out bit for bit as they went in: a NaN keeps its sign
    /// and payload, quiet or signalling, a zero its sign and a subnormal
    /// every bit, and silence is the default, positive zero.
    #[test]
    fn samples_come_out_bit_for_bit() {
        let odd = [
            f64::from_bits(0x7ff8_0000_0000_0001),
            f64::from_bits(0xfff8_dead_beef_cafe),
            f64::from_bits(0x7ff0_0000_0000_0001),
            f64::from_bits(0xfff4_0000_0000_0000),
            -0.0,
            0.0,
            f64::from_bits(1),
            -f64::from_bits(0x000f_ffff_ffff_ffff),
            f64::MIN_POSITIVE,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::MAX,
            -1.0 / 3.0,
        ];
        for (room, len) in [(1, 1), (2, 2), (5, 3), (13, 13), (40, 27)] {
            let here = format!("room for {room}, a delay of {len}");
            let sent: Vec<f64> = odd
                .iter()
                .cycle()
                .take(3 * len + odd.len())
                .copied()
                .collect();
            let mut line = DelayLine::with_room(room, len);
            for (i, &x) in sent.iter().enumerate() {
                let due = i.checked_sub(len).map_or(0.0, |j| sent[j]);
                assert_eq!(line.push(x).to_bits(), due.to_bits(), "{here}, push {i}");
            }
            let waiting: Vec<u64> = line.waiting().map(f64::to_bits).collect();
            let last: Vec<u64> = sent[sent.len() - len..]
                .iter()
                .map(|x| x.to_bits())
                .collect();
            assert_eq!(waiting, last, "{here}");
        }
    }

    /// A sample of a kind of its own, as the mixer's is: a level and a flag.
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Keyed {
        level: usize,
        keyed: bool,
    }

    /// Silence that is not all zeros, so a line that made it any way but as
    /// the default would show.
    impl Default for Keyed {
        fn default() -> Self {
            Self {
                level: usize::MAX,
                keyed: true,
            }
        }
    }

    /// Any kind of sample that is `Copy` with a default is carried whole,
    /// and its default is the silence: what comes out before the first
    /// sample is due, and what a line is made with, cleared to, and grown by.
    #[test]
    fn any_copy_kind_of_sample_is_carried_whole_its_default_the_silence() {
        let keyed = |i: usize| Keyed {
            level: i,
            keyed: i.is_multiple_of(2),
        };
        let silence = Keyed::default();
        for (room, len) in [(1, 1), (4, 3), (10, 10)] {
            let here = format!("room for {room}, a delay of {len}");
            let mut line = DelayLine::with_room(room, len);
            assert_eq!(whole_room(&line, room), vec![silence; room], "{here}");
            for i in 0..3 * len {
                let due = i.checked_sub(len).map_or(silence, keyed);
                assert_eq!(line.push(keyed(i)), due, "{here}, push {i}");
            }
            let last: Vec<Keyed> = (2 * len..3 * len).map(keyed).collect();
            assert_eq!(line.waiting().collect::<Vec<_>>(), last, "{here}");
            line.reset(room + 2, len);
            assert_eq!(whole_room(&line, room + 2)[room..], [silence; 2], "{here}");
            assert!(
                whole_room(&line, room + 2)[..len]
                    .iter()
                    .all(|&s| s != silence),
                "{here}: the room is seen to hold what was pushed"
            );
            line.clear();
            assert_eq!(
                whole_room(&line, room + 2),
                vec![silence; room + 2],
                "{here}"
            );
        }
    }

    /// A copy plays on just as the line it was taken from, and apart from
    /// it: the browser's mixer is copied to render a stretch again.
    #[test]
    fn a_copy_plays_on_as_the_line_it_was_taken_from() {
        let mut line = DelayLine::with_room(9, 5);
        for i in 0..7 {
            line.push(sample(i));
        }
        let mut copy = line.clone();
        let played: Vec<u64> = (7..20).map(|i| line.push(sample(i))).collect();
        let again: Vec<u64> = (7..20).map(|i| copy.push(sample(i))).collect();
        assert_eq!(played, again);
    }

    /// A plain model of a line: its room slot by slot, a delay over the
    /// first `delay` slots, and the slot the next sample goes in, which moves
    /// on one a push, back to the top at the end of the delay, and to the
    /// top on a reset. Where a line just made or one cleared puts its next
    /// sample, its docs leave open, and only a reset that keeps the room
    /// could tell: there the model is given the line's own slot.
    struct Model {
        slots: Vec<u64>,
        delay: usize,
        next: usize,
    }

    impl Model {
        fn new(room: usize, delay: usize) -> Self {
            Self {
                slots: vec![0; room],
                delay,
                next: 0,
            }
        }

        fn push(&mut self, sample: u64) -> u64 {
            let due = self.slots[self.next];
            self.slots[self.next] = sample;
            self.next += 1;
            if self.next == self.delay {
                self.next = 0;
            }
            due
        }

        /// From the next slot to the end of the delay, then from the top.
        fn waiting(&self) -> Vec<u64> {
            let (top, rest) = self.slots[..self.delay].split_at(self.next);
            rest.iter().chain(top).copied().collect()
        }

        fn clear(&mut self) {
            self.slots = vec![0; self.slots.len()];
        }

        fn reset(&mut self, room: usize, delay: usize) {
            let mut slots = vec![0; room];
            let kept = room.min(self.slots.len());
            slots[..kept].copy_from_slice(&self.slots[..kept]);
            *self = Self {
                slots,
                delay,
                next: 0,
            };
        }
    }

    /// Driven at random over delays and rooms from one sample to thousands
    /// — made with room to spare or none, pushed through many times round,
    /// cleared, and reset longer, shorter, roomier and tighter — a line
    /// gives back what a plain model of its room does at every push, and
    /// has the same waiting and the same room after every change.
    #[test]
    fn it_plays_as_a_plain_model_of_its_room_does() {
        let mut rng = SplitMix64::new(0x0DE1_A71E);
        // From one to 4 096: most often a few, now and then thousands.
        let size = |rng: &mut SplitMix64| {
            let most = [1, 2, 3, 4, 8, 16, 64, 256, 1_024, 4_096][rng.usize_in(0, 9)];
            rng.usize_in(1, most)
        };
        let mut next = 0;
        for round in 0..1_000 {
            let room = size(&mut rng);
            let (mut line, mut model) = if rng.usize_in(0, 3) == 0 {
                (DelayLine::new(room), Model::new(room, room))
            } else {
                let len = rng.usize_in(1, room);
                (DelayLine::with_room(room, len), Model::new(room, len))
            };
            model.next = line.at;
            for turn in 0..rng.usize_in(1, 10) {
                // From none to a few times round the delay.
                for push in 0..rng.usize_in(0, 3 * model.delay + 1) {
                    next += 1;
                    assert_eq!(
                        line.push(next),
                        model.push(next),
                        "round {round}, turn {turn}, push {push}"
                    );
                }
                match rng.usize_in(0, 4) {
                    0 => {
                        line.clear();
                        model.clear();
                        model.next = line.at;
                    }
                    1 | 2 => {
                        // Half the time in the room it has, as the mixer
                        // resets it.
                        let room = if rng.usize_in(0, 1) == 0 {
                            model.slots.len()
                        } else {
                            size(&mut rng)
                        };
                        let len = rng.usize_in(1, room);
                        line.reset(room, len);
                        model.reset(room, len);
                    }
                    _ => {}
                }
                let here = format!("round {round}, turn {turn}");
                assert_eq!(line.len(), model.delay, "{here}");
                assert_eq!(
                    line.waiting().collect::<Vec<_>>(),
                    model.waiting(),
                    "{here}"
                );
                assert_eq!(whole_room(&line, model.slots.len()), model.slots, "{here}");
            }
        }
    }
}
