# Per-character timing

Status: implemented. Stats → Letters shows each character's recognition
time and whether it is instant (ICR); the results screen shows the session's.

## What is measured

A character's **recognition time** is how long after its sound went out to
the speaker or headphones its key went in:

    recognition = key − end of the character's last element, sent to the output

- **The sound's moment** is when the character's last sample goes to the
  audio output, by the player's own clock. The delay from there to the ear
  cannot be known, so it is not guessed at: it is in every time alike, as is
  the moment after a character's last element before it can be told from a
  longer one.
- **The key's moment** is when that place of the answer box, or of a
  contest field, was first keyed: the keyboard's or the keypad's key, read
  before anything else the handler does, or, for a paddle letter, the first
  paddle contact of the letter (the keyer's `KeyedLetter::began`), which
  goes in only once its letter space has run out. A character typed where
  another was, typed again where one was taken out, or put in inside the
  text is a **correction** (`KeyLog`): it is no recognition of its own, but
  its place keeps the moment it was first keyed, which still says whether
  you were keeping up, so putting a slip right costs the characters around
  it nothing. Text typed for you (the guided setup's Show me) has no moment.

Nothing is polled for it and nothing waits on it: each key reads the clock
once and the box's `KeyLog` takes the moment as the text changes; each
send's character ends come from its plan, as the planner recorded them
(`PlaybackPlan::chars`), and reach the session as the send starts
(`SessionEvent::PlaybackStarted`), so a group answered before its send ends
keeps them. The moments travel with the text the box hands over
(`SessionEvent::Input`).

## What is kept

Each group keeps the raw facts its times come from, and the rule is applied
when they are read, so a later rule reads the whole history again instead of
mixing rule versions:

```json
"charTimes": { "ends": [180, 540, 900], "keys": [420, 760, null],
               "corrected": [1], "latencyMs": 40, "boxOpen": true }
```

- `ends` — where each character sent ended, in ms after the send's first
  sample went out.
- `keys` — when each position of the answer was first keyed, on the same
  clock; `null` where it cannot be: typed for you, before the send began, or
  after it was sent again.
- `corrected` — the positions holding a correction; left out when none do.
- `latencyMs` — the delay the device reports after the output. Used only to
  decide whether you were keeping up (below), never added to or taken off a
  time. Android reports none worth reading (its cpal host stamps the playback
  on another clock than the callback), so it counts as nothing there.
- `boxOpen` — whether the box took keys while the group was sent.

A group has `charTimes` only when its timeline can be trusted: its first send
played on time and once (a send the audio held up or parked, or one tried
again, is left out), and only when the rule could read a time from it: a
key to time, and, for more than one character, the box open as it was sent.
A send says it was held up as soon as that is known, whether it was heard
out or stopped by an answer (`SendNews::HeldUp`). The web player reads that
from its audio clock against the app's; the native player, which has no
clock to report, from what each send has played against the wall clock at
every callback. Keys after a group's second send began are `null`: it was
heard twice. A contest call or exchange token keeps the same record (see
Contest, below). The record is read leniently: a shape this build does not
know reads as untimed, never as a lost session.

A group whose first send could not be timed from says why instead, under
`untimed`: `"heldUp"` (the audio held it up on its way), `"sentAgain"`
(tried again from the start, heard in part already) or `"paused"` (paused
with its answer open). The first reason found is the one kept. It is
written only where there is one, which healthy audio never gives, and read
as leniently: a reason this build does not know reads as none. A session
run with Lock typing while sending on says so, `"typingLocked": true`, so
its results go by the setting it ran with, not by the one set since.

Older builds ignore `charTimes`. `timeToCompleteMs` and `avgResponseMs` are
still written for them, which need both.

## The rule

`recognise` (crates/cw-core/src/recognition.rs) times character *i* of what
was sent when all of these hold:

1. **It was typed right, in its place, and not as a correction.** Positions
   line up only where the answer is as long as what was sent; otherwise only
   those before the first difference, less a run of one letter up to it that
   the character at the difference, sent or typed, repeats (`KMM` typed `KM`:
   either `M` could be the one missed).
2. **It was heard before it was typed**, by a believable reaction: the key
   at least `REACTION_MIN_MS` (100 ms) after the sound. Sooner is a guess,
   or a letter read ahead from the word.
3. **You were keeping up.** The key for the character before it was in by
   the time this one reached you (`end + latency + KEEPING_UP_MS`, 100 ms),
   put right since or not: otherwise its time is the backlog. A group of
   more than one character is timed only if typed along (`typed_along`) —
   its first key in by then for its last character, or, however short the
   rest of the group, by `ALONG_FLOOR_MS` (400 ms) after its first
   character ended, with the delay and the 100 ms after that — and only
   with the box open: copied whole and typed after, it says how fast you
   recall, not recognise. With the box shut, only a group of one character
   can be timed.

   The floor is for the first key alone. Behind `ET` at 25 WPM the `T` is
   gone 288 ms after the `E`, sooner than most people key what they heard,
   so without it such a group could hardly be timed at all. A first key has
   no backlog behind it, so its time is its character's own, and one the
   floor lets in is timed at 500 ms at most, with the delay. A later key
   keeps the 100 ms: more would time the backlog.

A time past `RECOGNITION_MAX_MS` (3 s) counts as 3 s, as slow as a time
says, rather than leaving the slowest tries out.

## Where it is shown

- **Letters**: each character's median over its latest 40 timed tries, in
  its band once 8 are timed: fast, ICR, slow, too slow, at the edges set in
  Settings → Recognition (400, 600, 800 ms). A character counts as instant
  (ICR) in a fast or ICR band once it is building or mastered: a weak one's
  time is only that of the tries it got right. A character never timed says
  so: no estimate stands in for it.
- **Results**: the session's median over its timed characters, and how many
  were timed, where "Avg time" was. When under about a third of its
  characters were timed, it says why, as far as the session kept it
  (`session_untimed`): Lock typing while sending on as it ran, with a
  button to turn it off while it still is; the audio holding up most of
  its groups; how long after each group had ended its first key came,
  measured; or keys only once a group was sent again.

The paddles key a training group's box as the keyboard types into it: along
with the send when typing is not locked while it plays.

In group training the station you copy is always the strongest on the band
and apart in pitch from any calling across it, so it is timed under them; a
contest's callers are not, so one heard under another is not timed.

Every mode is timed by the same rule, contest runs included. The mode only
decides what is sent.

## What it does not change

The score keeps RufzXP's time term: how long the answer took after the send,
per character of the group (`timeToCompleteMs`). Every save has it, and the
history is scored again at every launch, so moving the score onto
recognition times would re-rank every board and record, and put sessions
from before and after on two scales. Auto level, the sampler and adaptive
speed read only accuracy and speed.

## Contest

A caller's call, or an exchange token, is timed the same way, in run time:

- its character ends come from the turn's own plan (the core plans every
  segment), from when its segment's sound went to the output (the turn's
  `output_from`, from `ContestEvent::TurnStarted`);
- its keys come from the field's key log (`Fields::call_keys`,
  `Fields::exch_keys`), each converted to run time at the key, so a pause or
  an Esc in between cannot move them; a token's are its word's in the
  Exchange field, and only for a field typed as it was sent (a cut number
  typed as digits stays untimed);
- it is timed from its first sending, when that was heard whole (not a rest
  carried on from a turn you keyed over), alone (no other caller, QRM or
  your own message sounding over it) and as one word in its send, with no
  other sending of it begun before (its caller's timeouts counted), in a
  turn that played on time and was not retried, with Show copy off. Keys
  from its second sending on are `null`, as a group's are, and so are keys
  after a pause: run time stands still while paused, and would leave the
  pause out of the time. A turn found held up after its QSO was logged
  takes the time back when the run is kept.

So a pile-up leaves most calls untimed, and Simple QSO's doubled
exchanges leave all of them untimed: both are honest. Simple QSO's caller
signs its call once, as MorseRunner's single callers do, so its call is
timed as any call sent alone is.
