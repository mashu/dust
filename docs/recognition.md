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
- **The key's moment** is when the answer box, or a contest field, first
  held that character in that place: the keyboard's or the keypad's key,
  read before anything else the handler does, or, for a paddle letter, the
  first paddle contact of the letter (the keyer's `KeyedLetter::began`),
  which goes in only once its letter space has run out. A character typed
  where another was, or where one was taken out, is a correction and has no
  moment (`KeyLog`). Text typed for you (the guided setup's Show me) has
  none either.

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
               "latencyMs": 40, "boxOpen": true }
```

- `ends` — where each character sent ended, in ms after the send's first
  sample went out.
- `keys` — when each position of the answer first held its character, on the
  same clock; `null` where it cannot be timed: retyped after a mistake, typed
  for you, or before the send began.
- `latencyMs` — the delay the device reports after the output. Used only to
  decide whether you were keeping up (below), never added to or taken off a
  time. Android reports none worth reading (its cpal host stamps the playback
  on another clock than the callback), so it counts as nothing there.
- `boxOpen` — whether the box took keys while the group was sent.

A group has `charTimes` only when its timeline can be trusted: its first send
played on time and once (a send the audio held up or parked, or one tried
again, is left out). Keys after a group's second send began are `null`: it
was heard twice. A contest call or exchange token keeps the same record (see
Contest, below). The record is read leniently: a shape this build does not
know reads as untimed, never as a lost session.

Older builds ignore `charTimes`. `timeToCompleteMs` and `avgResponseMs` are
still written for them, which need both.

## The rule

`recognise` (crates/cw-core/src/recognition.rs) times character *i* of what
was sent when all of these hold:

1. **It was typed right, in its place.** Positions line up only where the
   answer is as long as what was sent; otherwise only the run before the
   first difference, less any letter repeated at its end (`KMM` typed `KM`
   could be either `M`).
2. **It was heard before it was typed**, by a believable reaction: the key
   at least `REACTION_MIN_MS` (100 ms) after the sound. Sooner is a guess,
   or a letter read ahead from the word.
3. **You were keeping up.** The key for the character before it was in by
   the time this one reached you (`end + latency + KEEPING_UP_MS`, 100 ms):
   otherwise its time is the backlog. A group of more than one character is
   timed only if typed along — its first key in by then for its last
   character — and only with the box open: copied whole and typed after, it
   says how fast you recall, not recognise. With the box shut, only a group
   of one character can be timed.

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
  were timed, where "Avg time" was.

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
  `TurnStarted::output_from`);
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
  from its second sending on are `null`, as a group's are. A turn found
  late after its QSO was logged takes the time back when the run is kept.

So a pile-up leaves most calls untimed, and Simple QSO's doubled calls and
exchanges leave all of them untimed: both are honest.
