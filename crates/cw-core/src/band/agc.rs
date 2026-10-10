//! The receiver's automatic gain control.

/// The receiver's automatic gain control.
///
/// Not a limiter. A limiter squashes each sample where it stands and is done
/// with it; an AGC has a memory, and that memory is most of what a crowded
/// band sounds like. A crash arrives, the gain comes down within a couple of
/// milliseconds, and then it stays down and walks back up over the best part
/// of a second — so the whole band ducks behind the crash, the signal you were
/// copying with it, and comes back afterwards. Every operator knows that
/// sound, and the trainer had none of it.
///
/// Attack is fast because a receiver has to catch a crash before it is
/// deafening. Release is slow because that is what CW operators run: a fast
/// release pumps audibly between elements and makes the noise floor breathe in
/// time with the keying.
pub struct Agc {
    fast: f64,
    standing: f64,
    /// Samples left before the receiver trusts what it thinks the band is.
    warm_up: u32,
    gain: f64,
    attack: f64,
    release: f64,
    fast_fall: f64,
    standing_fall: f64,
}

/// How far above the band's standing level a peak has to be before the gain
/// comes down. Below this the receiver is riding a steady band and leaves it
/// alone.
pub const AGC_TRIGGER: f64 = 2.2;
/// Seconds for the gain to close on a level it has to duck to.
pub const AGC_ATTACK_SEC: f64 = 0.002;
/// Seconds for it to walk back up once the crash has gone.
pub const AGC_RELEASE_SEC: f64 = 0.45;
/// How quickly the fast follower forgets a peak.
const AGC_FAST_SEC: f64 = 0.06;
/// How long the receiver takes to decide what the band's standing level is.
const AGC_STANDING_SEC: f64 = 1.5;
/// How far down it will ever pull. Past this a crash is silencing the band
/// rather than riding over it, and a group would be lost rather than hard.
pub const AGC_MAX_DUCK: f64 = 0.25;

impl Agc {
    pub fn new(sample_rate: u32) -> Self {
        let sr = f64::from(sample_rate.max(1));
        let coefficient = |seconds: f64| (-1.0 / (seconds * sr)).exp();
        Self {
            fast: 0.0,
            standing: 0.0,
            // A fifth of a second listening before it judges anything. Without
            // it the standing level starts at nothing, the first sound of the
            // session stands infinitely above it, and the receiver spends the
            // next second and a half ducked for no reason.
            warm_up: sample_rate.max(1) / 5,
            gain: 1.0,
            attack: coefficient(AGC_ATTACK_SEC),
            release: coefficient(AGC_RELEASE_SEC),
            fast_fall: coefficient(AGC_FAST_SEC),
            standing_fall: coefficient(AGC_STANDING_SEC),
        }
    }

    /// Feed one sample of what the receiver is putting out; get back the gain
    /// everything should be riding at.
    pub fn next_gain(&mut self, sample: f64) -> f64 {
        let level = sample.abs();
        // A peak follower for the crash, and a long average *of that follower*
        // for the band it arrived on.
        //
        // Both halves of that are load-bearing. A standing level that is also
        // a peak follower leaps up with the crash, the ratio never moves and
        // the gain sits at one through everything. A standing level averaging
        // the raw samples instead measures something different in kind from
        // the peak above it — a steady sine already stands 1.57 times its own
        // mean — so the two are barely comparable and a quiet steady tone
        // reads as a crash. Averaging the follower makes the ratio one for any
        // steady signal whatever its shape, and large only when something has
        // just arrived.
        self.fast = level.max(self.fast * self.fast_fall);
        if self.warm_up > 0 {
            // Still learning the band: follow it exactly, and judge nothing.
            self.warm_up -= 1;
            self.standing = self.fast;
            self.gain = 1.0;
            return self.gain;
        }
        self.standing += (self.fast - self.standing) * (1.0 - self.standing_fall);

        // What matters is how far a peak stands above the band, not how loud
        // it is — which is what keeps this from quietly undoing the filter. A
        // narrower receiver passes less of everything, both followers come down
        // together, and the ratio between them does not move. An AGC watching
        // absolute level instead turns the gain back up as the filter closes
        // and hands back most of the quiet the filter just bought; measured,
        // it cost about five of the seven decibels.
        let standing = self.standing.max(1e-6);
        let excess = self.fast / standing;
        let wanted = if excess > AGC_TRIGGER {
            (AGC_TRIGGER / excess).max(AGC_MAX_DUCK)
        } else {
            1.0
        };
        // Down fast, up slowly — the asymmetry is the whole character.
        let coefficient = if wanted < self.gain {
            self.attack
        } else {
            self.release
        };
        self.gain = wanted + (self.gain - wanted) * coefficient;
        self.gain
    }

    pub fn gain(&self) -> f64 {
        self.gain
    }

    /// What the receiver currently reckons the band's standing level is.
    pub fn standing(&self) -> f64 {
        self.standing
    }
}
