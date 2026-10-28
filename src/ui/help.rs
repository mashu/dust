//! What each card's ⓘ opens: what its terms, colours and options mean, a
//! line each. Kept apart from the cards, so a card's code is what it draws
//! and its help reads as one text.

use cw_core::{
    BOARD_MIN_GROUPS, BOARD_RUN_MIN_S, BUILDING_MIN_ACCURACY, DROP_POINTS, ENDURANCE_MIN_SESSIONS,
    FORM_SESSIONS, GROUPS_A_STRETCH, HARD_PAIR_MIN_TRIES, KEEPING_UP_MS, MASTERED_MIN_ACCURACY,
    MASTERED_MIN_ATTEMPTS, MINUTES_A_STRETCH, REACTION_MIN_MS, RECENT_TRIES, RECOGNITION_MAX_MS,
    RECOGNITION_MIN_TRIES, RECOGNITION_WINDOW, Ranking, RecognitionBands,
};
use dioxus::prelude::*;

fn pct(share: f64) -> String {
    format!("{:.0}%", share * 100.0)
}

/// The colours and marks every bar card shares: where the paler stretch
/// runs, and what the notch is.
fn likely_and_notch(notch: String) -> Element {
    rsx! {
        dt { "Where it likely is" }
        dd {
            "The paler stretch past a bar runs to the top of the range the value is 80% likely to lie in. It narrows the more you hear: a wide one means the trainer is still learning."
        }
        dt { "The notch" }
        dd { "{notch}" }
    }
}

// Stats

/// What comes up next: how often each character goes wrong, against the
/// share wrong the level-up threshold leaves, `limit`.
pub fn next_up(limit: f64) -> Element {
    let pass = pct(1.0 - limit);
    let limit = pct(limit);
    rsx! {
        p {
            "One row a character of your set. The bar is how often you get it wrong: the trainer's best guess from every time you have heard it, whatever set it was sent in."
        }
        dl { class: "help-terms",
            dt { "Under your {limit}" }
            dd {
                "Green: wrong less than {limit} of the time. {limit} is what your Auto level threshold leaves: a session levels up at {pass} right (Settings → Auto level)."
            }
            dt { "Over it" }
            dd { "Copper: wrong more often than that. These hold your copy back: the ones to work on." }
            dt { "Still finding out" }
            dd { "Grey, dashed: heard too few times to tell, its likely range still more than 20 points wide." }
            {likely_and_notch(format!("Your {limit}: the line between under and over."))}
            dt { "Chance next" }
            dd {
                "How likely the next group is to hold the character. The trainer sends the ones you miss more often, and the ones it is least sure of (Settings → Character sampling)."
            }
            dt { "Not heard yet" }
            dd { "In your set but never sent: nothing to draw of them yet." }
            dt { "Not in the current set" }
            dd { "Practised before and not drawn now. What was learned of them is kept for when they come back." }
            dt { "Most work · Most likely · Taught order" }
            dd { "How the rows are sorted: the most work needed first, the likeliest next first, or as taught." }
        }
    }
}

/// Letter accuracy: where each character stands, its trend, and its
/// recognition time in the bands `bands` set.
pub fn letters(bands: RecognitionBands) -> Element {
    let RecognitionBands {
        fast_ms: fast,
        icr_ms: icr,
        slow_ms: slow,
    } = bands;
    let mastered = pct(MASTERED_MIN_ACCURACY);
    let building = pct(BUILDING_MIN_ACCURACY);
    rsx! {
        p {
            "Every character you have copied, over all your sessions. The bar is the share of the times it was sent that you copied it right."
        }
        dl { class: "help-terms",
            dt { "Mastered" }
            dd { "Green: {mastered} right or more." }
            dt { "Building" }
            dd { "Amber: {building} to {mastered} right." }
            dt { "Weak" }
            dd { "Copper: under {building} right." }
            dt { "Few tries" }
            dd { "Grey, dashed: sent fewer than {MASTERED_MIN_ATTEMPTS} times, too few to call." }
            {likely_and_notch(format!("The Mastered line, {mastered}."))}
            dt { "↑ 8 lately" }
            dd {
                "Your last {RECENT_TRIES} tries of it against the ones before, in points: up in green, down in copper, shown from 5 points either way."
            }
            dt { "Recognised in" }
            dd {
                "How soon you key a character once it has sounded: from when its last sample goes out to your speaker or headphones to your key, the median of its latest {RECOGNITION_WINDOW} timed tries. The key is the keyboard's or the keypad's, or for the paddles the first contact of the letter. A try is timed when you key it right, in its place, at least {REACTION_MIN_MS} ms after it sounded, while keeping up: the one before it keyed by {KEEPING_UP_MS} ms after this one reached you. A character put right after a slip is not timed, but costs the ones around it nothing. A group of more than one character is timed only when you key along, starting before it ends; keys after a group is sent again are not. A gap past {RECOGNITION_MAX_MS} ms counts as {RECOGNITION_MAX_MS} ms. Key each character the moment you hear it."
            }
            dt { "The bands" }
            dd {
                "Green under {fast} ms, fast; yellow under {icr} ms, instant character recognition (ICR): you know it as a sound rather than working it out; orange under {slow} ms; red from {slow} ms. Green and yellow count as ICR once a character is building or mastered: a weak one's time is only that of the tries it got right. Set them under Settings → Recognition."
            }
            dt { "About, so far" }
            dd { "Fewer than {RECOGNITION_MIN_TRIES} tries timed: the time so far, too few to put in a band." }
            dt { "Not timed yet" }
            dd {
                "With Lock typing while sending on, only groups of one character sent once (Sends per group 1×) are timed: turn it off (group training, Settings → Session shape) to key along, on the keyboard, the keypad or the paddles. In a contest run, a call or an exchange is timed when it was heard once and alone, with Show copy off. A send the audio held up is not timed, nor is what the guided setup types for you. Nothing stands in for a time: a character never timed says so."
            }
        }
    }
}

/// The Sampling tab's Speed card, against `aim` of characters right.
pub fn speed(aim: f64) -> Element {
    let aim = pct(aim);
    rsx! {
        p {
            "One row a speed the next group can come at, in words a minute. The bar is the share of characters you copy right at it, as the trainer's model of your copy reads it from every group you have heard: speeds you have heard little at are filled in from the ones around them."
        }
        dl { class: "help-terms",
            dt { "At or over your aim" }
            dd {
                "Green: {aim} right or better. Your aim is Aim for, in Settings: Comfortable asks 97% of characters right, Balanced 94%, Stretch 90%."
            }
            dt { "Within 10 points" }
            dd { "Amber: a little under it." }
            dt { "Further under" }
            dd { "Copper: more than 10 points under it." }
            {likely_and_notch(format!("Your aim, {aim}."))}
            dt { "Your aim · about 21 WPM" }
            dd { "The speed at which you copy your aim: Copy speed on the Overview." }
            dt { "Chance next" }
            dd {
                "How likely the next group is to come at about that speed. With Adapt speed to me the trainer leans towards where you copy your aim; without it, every speed in the range is as likely."
            }
        }
    }
}

/// The Sampling tab's Group length card, against `aim` of groups whole.
pub fn group_length(aim: f64) -> Element {
    let aim = pct(aim);
    rsx! {
        p {
            "One row a length the next group can have. The bar is how often you copy a group that long whole, without a mistake, as the trainer's model of your copy span reads it."
        }
        dl { class: "help-terms",
            dt { "At or over your aim" }
            dd {
                "Green: copied whole {aim} of the time or more. Your aim is Aim for, in Settings: Comfortable asks 85% of groups whole, Balanced 70%, Stretch 50%."
            }
            dt { "Within 10 points" }
            dd { "Amber: a little under it." }
            dt { "Further under" }
            dd { "Copper: more than 10 points under it." }
            {likely_and_notch(format!("Your aim, {aim}."))}
            dt { "Your aim · up to 4 characters" }
            dd { "The longest length you copy whole as often as your aim asks." }
            dt { "Chance next" }
            dd {
                "How likely the next group is to be that long. With Adapt group size to me the trainer leans towards the lengths you copy as your aim asks; without it, every length is as likely."
            }
        }
    }
}

/// Endurance: how copy holds up through a session.
pub fn endurance() -> Element {
    rsx! {
        p {
            "Whether your copy holds up as a session goes on. Your last sessions long enough to tell are cut into stretches of {GROUPS_A_STRETCH} groups, or {MINUTES_A_STRETCH} minutes of a contest run, and each stretch's character accuracy is averaged over them, every session measured against its own start. It shows once {ENDURANCE_MIN_SESSIONS} sessions are long enough."
        }
        dl { class: "help-terms",
            dt { "The first bar" }
            dd { "How you start: your accuracy over the first stretch." }
            dt { "The dashed line" }
            dd { "Your start's level, carried across the rest, to hold each stretch against." }
            dt { "A copper bar" }
            dd { "{DROP_POINTS:.0} points or more under your start: where you begin to tire." }
            dt { "Steady through · Fades after" }
            dd { "How far into a session you stay within {DROP_POINTS:.0} points of your start." }
        }
    }
}

/// Common confusions.
pub fn confusions() -> Element {
    rsx! {
        p {
            "Each line is a character you were sent and what you typed in its place, with how often, of every time that character was sent."
        }
        dl { class: "help-terms",
            dt { "The two in Morse" }
            dd { "The elements that differ are picked out in copper: the place your ear went wrong." }
            dt { "Heard once" }
            dd { "Mix-ups that happened only once are chips at the foot, so they do not crowd out the ones that keep happening." }
        }
    }
}

/// Hardest pairs.
pub fn hardest_pairs() -> Element {
    rsx! {
        p {
            "A pair is two characters one after the other: the one you heard first, then the one that went wrong. These are the pairs where the second goes wrong clearly more often after the first than it does anywhere else: the pair trips you, not the letter."
        }
        dl { class: "help-terms",
            dt { "Listed" }
            dd {
                "Once a pair has been heard {HARD_PAIR_MIN_TRIES} times, missed more than once, and goes wrong at least 10 points and half again more often than its second character does overall."
            }
            dt { "The bar" }
            dd { "How often the second character went wrong after the first; the mark on it, how often it goes wrong overall." }
        }
    }
}

/// The bigram grid.
pub fn bigram() -> Element {
    rsx! {
        p {
            "Each square is a pair: the character at the start of its row, followed by the one at the head of its column. Its colour is how often the second went wrong after the first: the deeper, the more often. An empty square was never heard; ▸ is the start of a group."
        }
        dl { class: "help-terms",
            dt { "A square" }
            dd { "Tap it for the pair's card: its tries, its misses, and how it compares with the character overall. Its row and column light up." }
            dt { "Taught order · Most errors" }
            dd { "Rows and columns as the characters were taught, or the ones that go wrong most first." }
        }
    }
}

/// Accuracy over time, against the level-up threshold `pass`.
pub fn accuracy_over_time(pass: f64) -> Element {
    let pass = pct(pass);
    rsx! {
        p {
            "Each point is a session's accuracy, the share of the characters sent that you copied right, oldest on the left."
        }
        dl { class: "help-terms",
            dt { "The dashed green line" }
            dd { "Your Auto level threshold, {pass}: a session over it counts towards the next level." }
        }
    }
}

/// Copy span.
pub fn copy_span() -> Element {
    rsx! {
        p {
            "The group length you copy whole about half the time, as the trainer's model reads it after each session of the character set shown."
        }
        dl { class: "help-terms",
            dt { "± and the band" }
            dd { "How sure the model is: the shaded band is one spread either side of its best guess." }
            dt { "Adapting" }
            dd { "Adapt group size to me is on, and steering the length of your groups by it now." }
        }
    }
}

/// Copy speed, at `aim` of characters right.
pub fn copy_speed(aim: f64) -> Element {
    let aim = pct(aim);
    rsx! {
        p {
            "The speed at which you copy {aim} of characters right, your Aim for, as the trainer's model reads it after each session of the character set shown."
        }
        dl { class: "help-terms",
            dt { "± and the band" }
            dd { "How sure the model is: the shaded band is one spread either side of its best guess." }
            dt { "Adapting" }
            dd { "Adapt speed to me is on, and steering the speed of your groups by it now." }
        }
    }
}

/// The practice calendar.
pub fn calendar() -> Element {
    rsx! {
        p { "One square a day. Tap a day for what you did on it." }
        dl { class: "help-terms",
            dt { "Volume" }
            dd { "Red through yellow to green the more characters you copied that day." }
            dt { "Accuracy" }
            dd { "Red through yellow to green as that day's copy gets better." }
        }
    }
}

/// What a board ranks by, in a line.
fn ranked(ranking: Ranking) -> &'static str {
    match ranking {
        Ranking::PerGroup => "what it earned a group",
        Ranking::Verified(_) => "its verified contest score",
        Ranking::CleanRate => "the QSOs it logged clean an hour",
    }
}

/// The score timeline, of a board ranked by `ranking`.
pub fn timeline(ranking: Ranking) -> Element {
    rsx! {
        p {
            "Each dot is a session on the board, oldest on the left, as high as {ranked(ranking)}. Point at a row of the board, or tap it, to mark it here."
        }
        dl { class: "help-terms",
            dt { "A larger dot" }
            dd { "It set a new record: better than every one before it." }
            dt { "Record" }
            dd { "Your best so far, stepping up at each new one." }
            dt { "Form" }
            dd { "The median of your last {FORM_SESSIONS}, so one bad day does not drag it down." }
        }
    }
}

/// The board, ranked by `ranking`.
pub fn board(ranking: Ranking) -> Element {
    let runs = BOARD_RUN_MIN_S / 60;
    rsx! {
        dl { class: "help-terms",
            dt { "Points a group" }
            dd {
                "A session's score is what its groups earn, more for copy at speed. Ranked by what it earned a group, a long session is no better than a short one for the same copy. A session goes on with {BOARD_MIN_GROUPS} groups or more."
            }
            dt { "Contest runs" }
            dd {
                "Under Contest, each contest has a board of its own, ranked by its own measure: the verified score, of what was logged clean, among runs set to the same length; or the QSOs logged clean an hour, fair between lengths. A run goes on once it ran {runs} minutes or more and logged a QSO clean."
            }
            dt { "Assisted" }
            dd { "A run with Show copy on: listed greyed, never placed, and kept out of the record and the form." }
            dt { "A tie" }
            dd { "The one set first holds the higher place." }
        }
        if ranking != Ranking::PerGroup {
            p { "This board ranks each run by {ranked(ranking)}." }
        }
    }
}

/// The Runs card.
pub fn runs() -> Element {
    rsx! {
        dl { class: "help-terms",
            dt { "Verified" }
            dd { "The contest score of what you logged clean: the callsign and exchange as the station sent them." }
            dt { "Raw" }
            dd { "The score of everything you logged, mistakes and all, as if no one checked." }
            dt { "Best" }
            dd { "The highest verified score of each run length, with Show copy off." }
            dt { "A row" }
            dd { "Tap it for the run's results: its log beside what was really sent." }
        }
    }
}

// Settings

/// Experience.
pub fn experience() -> Element {
    rsx! {
        p {
            "A starting point for where you are: Novice, Intermediate, Advanced or Expert sets the group size and the speed range, and nothing else. Change either of them and it reads Custom; pick a preset to go back to it. Your level, your session length, your history and every other setting are never touched."
        }
    }
}

/// Character set.
pub fn character_set() -> Element {
    rsx! {
        dl { class: "help-terms",
            dt { "Koch" }
            dd { "Letters and signs, unlocked one at a time in the order chosen: you start with two and add one as you copy them." }
            dt { "Digits · Mixed" }
            dd { "Digits only, or letters and digits together, each with a level of its own; Share of letters sets the mix." }
            dt { "Callsigns" }
            dd { "Real callsigns, as they are heard on the air. The tier sets how hard they come; each keeps what the ones below send." }
            dt { "Words" }
            dd { "Whole words, from the starter list or your own, one to a line." }
            dt { "Contest" }
            dd { "A contest run in place of groups: callers answer your CQ and you log their call and exchange." }
            dt { "Unlock order" }
            dd { "The order new characters come in: LCWO, MorseMania, CW Academy, alphabetical, or your own." }
            dt { "Level" }
            dd { "How many characters are in play. Auto level moves it for you." }
            dt { "Practice window" }
            dd { "Everything in play, or only the newest 3 or 5, to drill the ones just added." }
        }
    }
}

/// Your station.
pub fn station() -> Element {
    rsx! {
        p {
            "What you send in a run: your call, and the exchange your contest asks for, a name, state, zone, power or class and section. Callers check it as a real station would, and a run needs it set. Stored on this device only."
        }
    }
}

/// Session shape.
pub fn session_shape() -> Element {
    rsx! {
        dl { class: "help-terms",
            dt { "Groups per session" }
            dd { "How many groups one session sends; a contest run is timed in minutes instead." }
            dt { "Answer timeout" }
            dd { "How long a group waits for you to start answering before it moves on. Once you type, it waits for you." }
            dt { "Pause after each group" }
            dd { "Time to read the last group before the next is sent. Zero keeps contest pace." }
            dt { "Countdown" }
            dd { "Seconds counted down before the first group, to get ready." }
            dt { "Group size" }
            dd { "How many characters a group holds: one size, or a range drawn from for each group." }
            dt { "Adapt group size to me" }
            dd { "Within the range: longer groups while you copy them whole, shorter ones when you do not." }
            dt { "Aim for" }
            dd { "How hard adapting pushes: Comfortable keeps copy mostly clean, Stretch pushes to where you miss often." }
            dt { "Sends per group" }
            dd { "How many times each group is sent before the answer box opens, a word space apart." }
            dt { "Lock typing while sending" }
            dd {
                "Keeps you copying by ear rather than typing along. While it is on, a group longer than one character is typed only once it has been heard whole, so its characters cannot be timed (Stats → Letters)."
            }
            dt { "Key · Key speed" }
            dd { "How your [ and ] paddles key, and how fast: straight, iambic A or B, or Ultimatic." }
        }
    }
}

/// Speed.
pub fn speed_settings() -> Element {
    rsx! {
        dl { class: "help-terms",
            dt { "Character speed" }
            dd { "How fast each character is sent, in words a minute. A range draws each group's speed from it." }
            dt { "Effective speed" }
            dd { "Farnsworth: the characters stay fast and the gaps between them stretch, so the overall pace is slower." }
            dt { "Adapt speed to me" }
            dd { "Within the range: faster while you copy the characters, slower when you miss them." }
            dt { "Operator fists" }
            dd { "How far stations stray from a perfect keyer: heavier or lighter elements, longer or shorter dahs. Zero sends like a machine." }
            dt { "Extra word spacing" }
            dd { "Lengthens the gap between groups, in multiples of the standard word space." }
            dt { "Tie effective speed" }
            dd { "On, the gaps keep pace with the characters. Off gives you the Farnsworth controls." }
            dt { "Your speed" }
            dd { "In a contest, the speed you send at; the run screen's − and + move it." }
        }
    }
}

/// Tone & volume.
pub fn tone() -> Element {
    rsx! {
        dl { class: "help-terms",
            dt { "Volume" }
            dd { "How loud Dust plays everything: the band, the stations, your sending and the paddle's sidetone. For the band it is the receiver's AF gain, after the AGC. 100% is as loud as the strongest station plays without distorting at full RF gain; your device's volume does the rest." }
            dt { "Side tone" }
            dd { "The pitch you hear the Morse at. A range picks a fresh pitch for each group, as different stations sound." }
            dt { "Station strength" }
            dd { "How strong the station you copy comes in, against the band's noise and static: lower is harder to copy, not quieter overall. A range varies it from group to group." }
            dt { "Your sending" }
            dd { "In a contest, how loud you hear your own messages." }
            dt { "Full break-in (QSK)" }
            dd { "In a contest, hear the band between the elements you send, as a rig with QSK does." }
        }
    }
}

/// Keying envelope.
pub fn envelope() -> Element {
    rsx! {
        p {
            "How each dit and dah starts and stops. A tone switched on and off at once clicks; ramping it in and out softens the edges, as a real transmitter does."
        }
        dl { class: "help-terms",
            dt { "Rise time" }
            dd { "How long an element takes to reach full volume and to fade. It rises as the key goes down and fades as it comes up, as a transmitter does, so each element is heard as long as it is keyed. Short is crisp but clicks; long is soft but blurs fast dits into one another." }
            dt { "Edge smoothing" }
            dd { "The shape of that ramp: harder edges snap, smoother ones are gentler on the ear. Your paddle's sidetone opens and closes the same way, as a rig's follows its transmitter." }
            dt { "Dit · Dah · CQ" }
            dd { "Hear the envelope as it is now." }
        }
    }
}

/// Recognition: the bands of Letter accuracy's recognition times.
pub fn recognition() -> Element {
    rsx! {
        p {
            "How fast counts as what on Letter accuracy, where each character's recognition time is the median gap between its sound going out to your speaker or headphones and your key."
        }
        dl { class: "help-terms",
            dt { "Fast under" }
            dd { "Green: quicker than this." }
            dt { "ICR under" }
            dd { "Yellow: instant character recognition, the sound known at once rather than worked out." }
            dt { "Slow under" }
            dd { "Orange: slower than ICR, under this. Red from it on." }
        }
    }
}

/// Auto level.
pub fn auto_level() -> Element {
    rsx! {
        dl { class: "help-terms",
            dt { "Adjust the level for me" }
            dd { "Moves your level, how many characters are in play, by how your sessions go." }
            dt { "Accuracy threshold" }
            dd { "The share of characters right that counts a session as good. It is also the line What comes up next marks." }
            dt { "Sessions above to level up" }
            dd { "How many good sessions it takes to unlock a new character. Zero never levels up." }
            dt { "Sessions below to level down" }
            dd { "How many sessions under the threshold it takes to set the newest character aside. Zero never levels down." }
        }
    }
}

/// Character sampling.
pub fn sampling() -> Element {
    rsx! {
        p { "Which characters come round more often in your groups." }
        dl { class: "help-terms",
            dt { "Error-weight strength" }
            dd { "How much more often the characters you miss are sent: at 3, one you miss half the time comes 2½ times as often as one you never miss. Zero sends them all alike." }
            dt { "Coverage strength" }
            dd { "Brings round the characters that have come up less than their share in this session, so none goes missing." }
            dt { "Thompson sampling" }
            dd { "Draws each character's error rate from what the trainer believes of it rather than its average, so the ones it is unsure of, new ones above all, come up more." }
        }
    }
}

/// Band conditions.
pub fn band() -> Element {
    rsx! {
        p {
            "Everything you hear passes through one CW filter, the way it does in a real receiver: the band noise is a steady hiss pitched at the filter, and static and every dit ring in it for a moment."
        }
        dl { class: "help-terms",
            dt { "Filter width · shape" }
            dd { "The width is read between the points 6 dB down, as rigs quote CW filters. Narrow it and less noise gets in while a station on your pitch stays as strong, so its S/N rises; it rings longer, and a station off your pitch fades. Sharp is a modern DSP filter, Soft a rounded DSP shape that barely rings, Peaked adds a rig's audio peak at your pitch." }
            dt { "Stations" }
            dd { "How many call at once. More than one is a pile-up, spread either side of your pitch." }
            dt { "Band noise" }
            dd { "The steady hiss of the band; its S/N is as heard through your filter." }
            dt { "QSB fading" }
            dd { "Slow swells and dips of each station, every one on a path of its own, down as far as the depth says. With the AGC on a strong station holds nearly steady as it fades, and the hiss rises around it, as on the air." }
            dt { "QRN static" }
            dd { "Lightning: the crackle of distant storms and the crash of a near one." }
            dt { "RF gain" }
            dd { "Turns down everything reaching the receiver, ahead of the AGC: the noise, the static and every station alike. With the AGC on, the strong stations it was riding stay nearly as loud while the noise and the weak ones drop; with it off, the whole band gets quieter. The volume does not make it up, as on a rig." }
            dt { "AGC" }
            dd { "The receiver rides its gain, as a rig's AGC does: every station over the threshold comes out within a few dB of the others, a weak one nearly as loud as a strong one, and the hiss ducks under a strong station and comes back between overs. Off, every station keeps its own level." }
            dt { "AGC decay" }
            dd { "How fast the gain comes back once a signal has gone. Fast is back by the next word; Slow holds through an over and comes back over a second or so, so a weak station straight after a strong one starts quiet." }
            dt { "AGC threshold" }
            dd { "How strong a signal has to be before the AGC acts. Lowest starts it just above the quietest hiss, so every station you can copy is evened out; raised, it acts only on stronger ones, and the weaker keep their own level. The volume follows, so the strongest station stays as loud." }
            dt { "Live preview" }
            dd { "Loops a CQ through the band as it is set, loud, medium and weak in turn, so you hear each change and what the AGC does with each of them." }
        }
    }
}
