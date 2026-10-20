# Contest mode

Status: being built phase by phase (§14). Phase 0, history that lasts, has
landed: Contest exists as a mode no one can pick yet, runs written by later
builds are read and kept, and History names every session's mode. Phase 1's
cw-core foundations are in: the WPX rules row, contest calls and their
parsing, exchanges and cut numbers, ESM, the run clock, callers' serials,
checks, both scores, the run's log layout, and the contest settings. So is
its engine: single callers, turns and `plan_turn`, the run machine and what
a run stores, with operator tests and a soak. The app runs it from start to
results (`src/contest_runtime.rs`) on a plain run screen
(`src/ui/contest.rs`), with pause, the hidden-page pause and the sound
failure pause; the screen's full layout, the keypad, the contest results
and the settings cards come next. Until then a run that worked nobody goes
back Home with its message, not to a panel with Run again (4.2), and what
the run announces shows at once, not held till the air is silent (§7).
Nobody can pick Contest yet (§14).

This is the plan for contest and pile-up training in Dust. References are
`file:line` in this repository unless they name MorseRunner CE ("MRCE",
the `w7sst/MorseRunner` sources) or Morse Walker. Calls in examples are
placeholders. You are DL1ABC in DX-side examples and K1ABC in North
American ones.

## 1. Summary

Contest is a sixth mode in the Character set card, after Koch, Digits,
Mixed, Callsigns and Words. Pick it and the card shows contest types
instead of an alphabet: Sprint, ARRL DX, WPX, CQ WW, Field Day and Simple
QSO. Your callsign and exchange go on a Your station card, once.

A run is a timed session on the training screen. You call CQ, a station
answers, you send its call and your exchange, copy its exchange, and send
TU. Enter always sends what the QSO needs next (Enter Sends Message, as in
MorseRunner). Again sends `?` on the air, at a cost in points. The screen
shows the clock, QSOs, rate and your speed with − and +. Pause silences
everything, and a hidden page pauses the run. A live log marks each
mistake; the results, also opened from History, give a raw and a verified
contest score.

Each copied call and exchange field is stored as an ordinary group of a
`SessionResult`, and so is each call you worked but lost, so every stat
counts contest practice with no special code. A compact log rides beside
the groups. Older builds keep the run as callsign practice.

The first release offers WPX, Simple QSO and Sprint, with single callers
and pile-ups (end of phase 2, §14).

**User stories**

1. I pick a contest type instead of an alphabet and see what its exchange is.
2. I enter my callsign, and the fields my contest needs, once.
3. I run 10 minutes against one caller at a time and learn the order of a
   contest QSO. Then I work pile-ups, and `K5?` brings back only the
   stations that fit.
4. When I miss something I press Again and the station sends it again. When
   I send a wrong call, the station corrects me.
5. I pause and resume; the clock stops and the band goes quiet while paused
   or while the phone is locked.
6. I see time, QSOs, rate and speed while I run, and change speed with − / +.
7. Afterwards, and later from History, I see the log with corrections, the
   rate and both scores.
8. My runs count in the calendar, streak, letters and mistakes, with a
   Contest pill on Copy speed and the board. Nothing I did before disappears.
9. I can run a contest on a phone with the keypad, or on a desktop with
   MorseRunner's keys.
10. As a beginner I can hear a whole QSO first, and a one-line hint says
    what Enter will do next.

## 2. Where it lives

The user asked that "instead of alphabet ... we don't show alphabets but
contest types". The Character set card is where Dust already chooses what
is practised, so Contest goes there.

**The picker.** `CharSetMode` gains `Contest`
(`crates/cw-core/src/settings/mod.rs:116-144`). `CHAR_SET_MODES` gains
`(CharSetMode::Contest, "Contest")` (`src/ui/settings_charset.rs:51-58`)
when the first release ships (§14). `.segmented` wraps
(`assets/styles.css:785-803`); at 420 px and below the six segments sit in
a 3 + 3 grid.

**The card in Contest mode.** The title stays "Character set", so its Reset
still means what it says; the note (`settings_charset.rs:123-142`) reads
"Contest · CQ WPX". The body shows:

- one pill per contest, once its phase ships (a control that does nothing
  is removed, `src/app.rs:1606-1635`);
- "You send" and "You copy", and the QSO as a dialogue with You and They
  labels, rendered with your call (or `<your call>`);
- "Callsign tier (1–6)", the stored `callsignLevel` shared with Callsigns,
  which shapes the callers' calls;
- from phase 2, the run style (Single caller or Pile-up, "1–3 callers · set
  on the Band card") and presets (§8).

The alphabet controls are not drawn: the `if` blocks at
`settings_charset.rs:171-275` gain a Contest arm.

**Your station.** A new card under it, in Contest mode only, with its own
reset: your callsign and only the fields the chosen contest sends. Under the
call it shows what was read ("Germany · Europe · CQ zone 14 · you are DX")
or a plain error ("needs a digit", "unknown prefix"), and "No licence yet?
Make up a call from your country's prefix, a digit and two or three
letters. Stored on this device only."

**Home** (`src/ui/home.rs:128-216`) keeps its frame. The hero's eyebrow
reads "Contest", the level chip "CQ WPX", and field chips (`CALL` `NR`)
replace the pool chips, with "You send 5NN + serial, as DL1ABC". While
`myCall` is empty the hero holds an inline callsign field instead. Specs
show the callers' WPM range, minutes per run and "Single caller" or "1–3
callers". Start reads "Start contest" (id `btn-start-training`). Contest has
its own first-session card ("Add your call, press Start contest; Enter sends
what the QSO needs next"). The level tile shows the callsign tier, and
`AutoLevelCard` is hidden, as for Words (`crates/cw-core/src/auto_level.rs:220-229`).

**Listen** treats Contest like Callsigns (`src/ui/listen.rs:30-222`), as
does the one fork in group generation (`sampling.rs:503-507`). Phase 2 adds
"Hear a QSO".

**Navigation.** A run uses `Screen::Training` and `Screen::Results`
(`src/state.rs:19-26`), with the hidden nav, the refused `go_to`
(`state.rs:368-371`) and no settings saves mid-run. Three things change:

- Exit and the tour's Discard send a `SessionEvent` (`app.rs:184`,
  `tour.rs:222`) that only a group `LiveSession` receives
  (`session_runtime.rs:232-241`). Both go through new `finish_run(ctx)` and
  `abort_run(ctx)`, which reach whichever machine is live.
- Keep-awake follows "a run is live" (countdown or running) instead of the
  screen (`keep_awake.rs:17-27`), so a paused run lets the screen sleep.
- The tour targets group inputs (`tour.rs:231-240`), so its button is
  hidden in Contest mode until phase 5.

## 3. Contest catalogue

A contest is one row of a `ContestRules` table in cw-core: the CQ and TU
texts, your exchange, whether RST and a serial are part of it, the copied
fields per side, which callers answer, and the scoring rule. A new contest
is a row plus its data. In all of them you run (call CQ); callers send RST
as `5NN` (`Station.pas:529`), prefilled 599 and never checked, as loggers
do, except in Simple QSO; in pile-ups TU becomes `TU <your call>` every
third QSO, as in MRCE's Pile-Up and WPX modes (`Contest.pas:468-474`); and
you type into two fields, Call and Exchange, as
in the screenshot, with exchange items in any order and a later item of a
kind replacing an earlier one (11.7).

| | Sprint | ARRL DX | WPX | CQ WW | Field Day | Simple QSO |
|---|---|---|---|---|---|---|
| Real contest | NCJ NA Sprint | ARRL International DX | CQ WPX | CQ WW DX | ARRL Field Day | an everyday QSO |
| RST, serial | no, yes | yes, no | yes, yes | yes, no | no, no | yes, no |
| You copy | NR NAME ST | ST or PWR | NR | ZN | CL SEC | RST NAME |
| Callers | US, Canada | the other side | world | world | US, Canada, ~1% DX | world |
| Phase | 2 | 3 | 1 | 3 | 3 | 1 |

### WPX (`wpx`), the default

- **On the air.** `CQ DL1ABC TEST` (`Contest.pas:466`) · `K5XYZ` ·
  `K5XYZ 5NN TT7` · `TU 5NN 214` · `TU`. They may also send `R 5NN 214`.
  Logged: K5XYZ, NR 214. Your serial starts at 1, padded to three digits and
  cut (`TT7`, `1N5`), as MRCE sends the user's (`Station.pas:482, 541-547`).
- **Rules.** One point per QSO × different WPX prefixes. The real contest
  scores 1–6 points by continent and band; MRCE simplifies the same way
  (`CqWpx.pas`, `ExtractMultiplier`), so scores compare with the 10-minute
  runs its users share. Your fields: callsign.

### Simple QSO (`simpleQso`)

- **Purpose.** A first real QSO for a learner. The template is Dust's own, a
  short ragchew-style exchange that borrows `BK` and `73 EE` from Morse
  Walker's POTA mode (`mw/src/js/modes.js:85-100`).
- **You send** `CQ CQ DE <my> <my> K`, then
  `<his> DE <my> UR 599 599 NAME <name> <name> BK`, then `TU <his name> 73 EE`.
- **They send** `<my> DE <his> <his> K`, then
  `R TNX UR 579 579 NAME BOB BOB BK`, then `73 EE`, heard but not logged.
  Logged: RST 579, NAME BOB.
- **Rules.** TU does not bring the next caller; you call CQ again. No contest
  score: QSOs, minutes per QSO and accuracy per field. Your fields:
  callsign, name.

### Sprint: NA Sprint (`sprint`)

The screenshot's "Sprint Contest" with "Serial Nr, Name, QTH" is the NCJ
North American Sprint.

- **On the air.** `CQ NA K1ABC` · `W7XYZ` · `W7XYZ K1ABC 7 MIKE MA` ·
  `K1ABC 145 BOB OR` · `TU K1ABC`. Logged: NR 145, NAME BOB, ST OR. Your
  serial is sent plain.
- **Simplified rules.** No QSY: the real CQ station must move after each
  QSO, but Dust keeps you on one frequency, so you sign last (`TU K1ABC`)
  as the station keeping a frequency does; the caller's call went out when
  it called. One band, so a call is a dupe after one QSO. 1 point ×
  multipliers: US states (KL7 and KH6 count), DC, and the 13 provinces and
  territories (AB BC MB NB NL NS NT NU ON PE QC SK YT); other NA countries
  count too, but Dust never generates them. The card states these.
- **Your fields.** Name; state, province, or `DX` outside North America.

### CQ WW (`cqww`)

- **On the air.** `JA1XYZ 5NN A4` (your zone 14, cut as MRCE cuts zones) ·
  `TU 5NN 25`. Logged: ZN 25.
- **Rules.** One band. Points: 0 for your own country, 1 for your own
  continent, 2 between North American countries, 3 for another continent.
  Multipliers: zones plus DXCC countries, Alaska and Hawaii among them (MRCE
  folds them into the USA, `CqWW.pas:260-263`). No WAE countries: the card
  lists that beside "one band".
- **Your fields.** Callsign; your zone comes from your call (and state or
  province in North America), with an override.

### ARRL DX (`arrlDx`)

The pill reads "ARRL DX", the best-known "DX contest". The card says which
side you are on: "You are DX: you work US and Canada only". IARU HF, under
later contests, is the world-wide one.

- **Sides.** W/VE stations (the 48 contiguous states, DC and Canada) work
  only DX; DX works only W/VE. Your side comes from your call (§11.2).
- **As DX.** `W7XYZ 5NN 1TT` (100 W, cut) · `R 5NN OR`. Logged: ST OR.
- **As W/VE.** `G4XYZ 5NN MA` · `TU 5NN KW`. Logged: PWR KW.
- **Rules.** 3 points per QSO, multipliers counted once on Dust's one band.
  If you are W/VE: DXCC entities. If you are DX: the 48 states, DC, NB NS QC
  ON MB SK AB BC NT NL LB NU YT PE, 63 in all (the ARRL DX rules). VO2
  stations send `LB`, a multiplier of its own. Alaska and Hawaii are DX.
- **Your fields.** State or province (with LB), or power.

### Field Day (`fieldDay`)

- **On the air.** `CQ FD K1ABC` (`ArrlFd.pas:434`) · `W7XYZ` ·
  `W7XYZ 1D EMA` · `R 3A OR` · `TU`. No RST. A class is the number of
  transmitters plus a letter A–F.
- **Rules.** 2 points per CW QSO (`ArrlFd.pas:493-497`). No power multiplier
  and no bonus points: those score the entry, not the copying.
- **Your fields.** Class (default `1D`) and section, defaulted from your call
  (§11.4).

**Later contests,** in order of value: K1USN SST (with Farnsworth), CWops
CWT, NAQP, IARU HF, Sweepstakes, and an HST-style preset with fixed
settings so scores compare between people.

## 4. Operating flow

### 4.1 Run styles

- **Single caller** (phase 1, default). One station answers each CQ, and
  each TU, which works as a CQ. Its pitch is within ±50 Hz of the centre,
  as in MRCE's Single Calls (`DxStn.pas:102-105`).
- **Pile-up** (phase 2). After each CQ or TU, new callers join until the
  number calling reaches a fresh draw from the Band card's stations range
  (1–5). Callers not worked keep calling while their patience lasts.
- **No Search & Pounce,** which neither MorseRunner nor Morse Walker
  simulates.

### 4.2 The machine

`ContestMachine<S: CallSource>` sits in cw-core beside `SessionMachine` and
follows its pattern: pure `apply(event, now) -> Vec<effect>`
(`machine.rs:1-7, 247-271`), timers cancelled by forgetting their ids
(`machine.rs:184-199`), and randomness from `SplitMix64` forks per caller,
so a seed and an event list give one run. Its state:

- `phase`: `Countdown{left}`, `Running`, `Paused{why: You|Hidden|Sound}`,
  `Finished` or `Aborted`, with a `RunClock` (§6);
- `air`: `Silent`, or a `Turn` (§12) and when its sound reached the ear;
- `qso`: the call last sent (MRCE's C), whether the station you work took
  your exchange (N), whether it corrected you, who you are working, and
  when each item was heard and asked for;
- `callers`: call, place, true exchange, a voice drawn once
  (`timing/station.rs:6-11`), pitch, level, fade offset, skill 1–3,
  patience, and MRCE's state (`DxOper.pas:17-88`): `NeedQso`, `NeedNr`,
  `NeedCall`, `NeedCallNr`, `NeedEnd`, `Done`, `Gone`;
- the log, your next serial, the run's speed offset, one id counter.

```rust
pub enum ContestEvent {
    Key { key: OpKey, fields: Fields }, // after a paddle letter finishes
    TurnStarted { id: u64, heard_from: MonoMs }, // each attempt, retries too
    TurnEnded { id: u64 },
    TurnCancelled { id: u64 },  // a stop or a newer send: always ignored
    TurnFailed { id: u64 },     // the sound failed after every retry
    TimerFired { id: u64 },
    Pause { why: PauseWhy, at: MonoMs },
    Resume,
    Speed { delta: i32 },
    FinishNow,
    Abort,
}
pub enum OpKey { Esm, Cq, Exchange, Tu, MyCall, HisCall, HisCallAndExchange,
                 B4, Again, Nil, NrQuery, TuAndLog, LogOnly, Wipe, StopTx }
```

The machine needs the fields only at a key press. The runtime maps
`Err(AudioError::Cancelled)` to `TurnCancelled` and any other error to
`TurnFailed`, as `play()` tells the two apart (`session_runtime.rs:350-354`). The effects are
`PlayTurn(Turn)`, `StopAudio`, `ReleaseAudio`, `StartTimer{id, ms}`,
`ClearFields`, `Focus(Field)`, `Announce(text)`, `PersistAndShowResults`,
`ShowNothingKept` and `AbortToHome`. Every apply that emits `StopAudio` or
`ReleaseAudio` also sets `air = Silent`.

| In | Event | Effects → next |
|---|---|---|
| Countdown n | TimerFired | 1 s → n−1. At 0 the clock and run-end timer start and your first CQ goes out → Running |
| Countdown | Pause | timers forgotten, `ReleaseAudio` → Paused; Resume restarts the countdown |
| Running, Silent | Key | ESM (4.4) or the key picks the message; a logging key logs (4.8); replies are decided → `PlayTurn` (new id) |
| Running, Turn | Key but StopTx | a continuation (5.2) → `PlayTurn` (new id); a key that sends nothing (Wipe, Shift+Enter, Enter on a bad exchange) leaves the turn playing, and Shift+Enter first counts what of it has reached you, as a key that sends does |
| Running, Turn | StopTx (Esc) | `StopAudio`; C and N reset for a call or exchange of the message keyed last not fully sent (`Main.pas:884-891`), not for one carried from an earlier key, heard then; the callers hear only what of the message keyed last went out whole, and a QSO it lost or a caller it brought goes with it (5.2) → Silent |
| Turn(id) | TurnStarted{id} | `heard_from` re-stamped; no effects, no limit check |
| Turn(id) | TurnEnded{id} | all segments count; the patience timer starts if anyone waits → Silent |
| Turn(id) | TurnFailed{id} | time since the turn first started is credited as paused, but not time the clock was read at since (6.3), so the run bar steps back by the retries; every caller in the turn sends again after Resume, as it was keyed, so a repeat you asked for is charged once heard; timers forgotten; `ReleaseAudio` → Paused{Sound} |
| any | TurnCancelled, or another id | ignored |
| Silent | TimerFired (patience) | waiting callers call again (4.3); if the station you work has left, "K5XYZ has gone" is announced and Enter starts afresh, as after a CQ → a turn of replies |
| Running | Pause{why, at} | `StopAudio`, `ReleaseAudio`; segments heard by `at` count. A send it cut before it began goes again after Resume as it was keyed, so a repeat you asked for is charged once heard; one it cut after it began goes again, as no send you asked for, after a hidden page, but after a pause you chose it is owed on the caller's timeout, as one heard whole. A pause with nothing on the air keeps who an earlier pause or failure cut short. Timers forgotten; clock paused from `at` → Paused |
| Paused | Resume | clock resumes; run end re-armed with the time left; a 1 s resume timer → Running |
| Running | the resume timer | callers the pause or failure cut short send again, as the rows above say; the others wait for you afresh, as after a turn, so what was heard whole comes again only on a timeout or at your asking: a pause is no free `?`. A key that sends forgets the timer; one that sends nothing (Wipe, Shift+Enter, Esc with nothing on the air) leaves it to bring the callers back |
| Running | run-end timer, or the limit passed on any event but TurnStarted (a look at the clock, which stamps nothing, 6.3) | → Finished. While the turn on the air may be retrying its sound (an attempt after the first has started, or the one playing is past its length and 2 s), only what keeps its reading ends the run (its end, a key, End run): its failure gives the retries back (5.2), and the run end waits until its last attempt would have failed |
| live | Speed{±2} | the offset moves for your next message and later callers |
| live | FinishNow | → Finished |
| any | Abort | `StopAudio`, `AbortToHome` → Aborted |

**Finished** emits `StopAudio`. A logged QSO is final and checked, its TU
cut or not; any other open QSO becomes a lost copy (9.1). With no groups
(no station worked, though a NIL may have been logged with nobody heard),
nothing is stored and `ShowNothingKept` shows "No stations worked — nothing
kept" with Run again; otherwise `PersistAndShowResults`.

### 4.3 Callers

The callers follow MRCE's model, re-written from its behaviour, credited.

- **Reply delay** 0.1–0.6 s after your message (`DxOper.pas:225-232`).
- **Reply timeout** `(6 − skill)` s, Gaussian, within ±50%
  (`DxOper.pas:282-287`). Each timeout costs a unit of patience and the
  caller sends again.
- **Patience** while calling is `3 + Rayleigh(3)` tries, 4–14 with a mean
  near 6 (`DxOper.pas:496-498`). Each message a caller answers while it
  calls (CQ, TU, `?`, a partial) costs a try too, as MRCE's `DecPatience`
  (`DxOper.pas:709, 825`), so CQ after CQ moves the frequency on to
  someone else.
- **The station you work stays** while you keep keying, and leaves after 3
  silent timeouts in a row, as MRCE's Caller Stays (`DxOper.pas:319-335`).
- **QRS** (phase 2, habits `easy`). After 3 incomplete tries (4 for a skilled
  caller) a caller slows by 12–20% by skill, 2% more from the fourth
  (`DxOper.pas:343-375`).
- **Calling twice.** 10% of pile-up callers send their call twice; single
  callers never do (`DxOper.pas:500-502`).

| You send | A caller still calling | The station you work |
|---|---|---|
| CQ or TU | calls; once its call has begun to reach you, that is a call you asked for again (9.1) | after TU: `Done`. After CQ: `Done` if it has already sent its exchange (`NeedEnd`); before that the QSO fails and it leaves (`DxOper.pas:705-711`) |
| `<call> <exch>`, exact | falls silent | sends its exchange |
| `<call> <exch>`, close (4.7) | falls silent | corrects you: `DE K5XYZ` or `K5XYZ K5XYZ` |
| `<call> <exch>`, no match | calls again after its timeout | — |
| `?`, `AGN`, `NR?` | repeats its call, unless it heard you work someone else since it last called and the Call field is not empty, as MRCE's `osNeedPrevEnd` (`DxOper.pas:744-748, 793`) | re-sends what it owes: call or exchange. After someone else's call, neither answers |
| partial `K5?` | only the best matches answer; your exchange after a partial it answered, in that message or a later one, is for it: it takes it and puts its call right (`NeedCall`), as MRCE's `osNeedCallNr` (`DxOper.pas:734-742, 768-776`) | sends its call, as MRCE's `mcAlmost` (`DxOper.pas:734-742`): `NeedNr` → `NeedCallNr`, `NeedEnd` → `NeedCall` |
| B4, NIL (phase 2) | NIL costs a patience | leaves |

From phase 2, in pile-ups only:

- **Collisions.** A caller still sending when your message starts does not
  hear it and keeps sending over you, except CQ, TU and NIL, which are heard
  from a fragment (`DxStn.pas:240-253`).
- **Missed exchanges.** 10% of the time a caller misses your exchange and
  sends `NR?` or `AGN` (`DxOper.pas:762-770, 844-849`).

A single caller you talk over stops and hears you, kinder to a learner.

A caller hears each of your messages whole, in order, when you key it, and
its reply is decided then (5.1); Esc takes back what had not gone out whole
(5.2). A key during a turn sends the rest of your message again, which the
callers heard when it was keyed, so they hear only the new message: an
exchange keyed then, after someone else's call, goes to the station you
work. A station that hears its call alone (F5) waits for your exchange and
asks for it with `NR?` or `AGN`, as MRCE's `NeedNr` does. The patience timer
runs to the first waiting caller's timeout; each caller draws its own.

### 4.4 Enter Sends Message

ESM follows MRCE's `ProcessEnter` (`Main.pas:1162-1224`). **C**: the Call
field equals the call last sent. **N**: the station you work took your
exchange; one sent with a call nobody took for its own is owed still.
**V**: the call is whole (`parse_callsign` accepts it), with no `?`. **E**:
the Exchange field is Empty, Missing (some fields, not all), Invalid (a
token that cannot be read, or a value off its list) or Complete (11.7). MRCE's Q
(RST typed) is always true, because RST is prefilled; in Simple QSO the RST
is part of E.

| Situation | Enter sends | Key label |
|---|---|---|
| Call empty | CQ | `CQ` |
| your TU without logging (F3) went to this call, V, E Complete | TU, and logs that QSO, as MRCE's C and N still hold after F3 | `TU & log` |
| C, V, and the station corrected that call | nothing; focus to Call, as a `?` would only bring the correction again | `Fix call` |
| not C, or not V, or (not N and E Empty) | his call as typed, then… | `Send K5XYZ`, or `Ask K5?` |
| …not N and V | …your exchange | `Send K5XYZ + exch` |
| N, V, E Empty or Missing | `?` | `? Again` |
| N, V, E Invalid | nothing; "Reads as" shows the fault | `Fix exchange` |
| E Complete, V, and (C or N) | TU, and logs | `TU & log` |

The rows that fire go out as one message: `K5XYZ` typed sends
`K5XYZ 5NN TT7`; then `?` while the exchange is empty; then `TU`, logging.
A CQ or a TU ends the QSO for Enter as logging does, and so does a key
pressed with the Call field empty, as MRCE's `Edit1Change` resets C and N
when the field empties (`Main.pas:2063-2068`), and the station you work
leaving. F3 keeps only the call it went to, so Enter on that call logs the
QSO it closed, and any other call starts afresh.
A call corrected after your exchange sends `K5XYZ TU` and logs. A partial
always goes out again as typed, alone. Where MRCE refuses an invalid
exchange (`Main.pas:1210-1216`), Dust also sends nothing: the fault is your
typing. When Enter does not log and V holds, focus moves to Exchange, as
MRCE's `MustAdvance` (`Main.pas:1222-1223`); after logging it returns to
Call. `esm(&qso, &fields, &rules, side) -> Esm { message, label, hint, focus }`
is pure: the machine runs it on the key, the screen on its own field text.
It takes your side because ARRL DX's exchange depends on it, as
`parse_exchange` does. When rows combine, the label names each part:
`Send K5XYZ + ?`, `Send K5XYZ, TU & log`, `Send exch, TU & log`.

### 4.5 Keys and buttons

| Action | Desktop | Phone | Phase |
|---|---|---|---|
| ESM | Enter | the big key, labelled as in 4.4 | 1 |
| CQ | F1 | `CQ` | 1 |
| Again: `?` (4.6) | F7 | `? Again` | 1 |
| Stop sending; wipe the fields | Esc or Alt+S; Alt+W | —; `Wipe` | 1 |
| Next field | Space in Call; Tab as on any page | `␣ Field`, or a tap | 1 |
| Speed ±2 WPM; pause; end run | PgUp/PgDn; Alt+P; Alt+E | − / +; `Pause`; `End run` | 1 |
| Your exchange; TU without logging (as MRCE) | F2; F3 | — | 1d |
| His call; his call + exchange | F5; `;` (Insert also works) | — | 1d |
| TU and log; log without sending | `.` `,` `+`; Shift/Ctrl/Alt+Enter | (ESM) | 1d |
| Your call, B4, NIL, NR? | F4, F6, F8, F12 | `B4` on a dupe | 2 |

- F11 stays fullscreen (`app.rs:140-147`), so Wipe is Alt+W (Ctrl+W closes
  a tab), and `[` `]` stay the paddles (`paddle.rs:113-125`). Web
  fullscreen takes Esc, so the run calls `navigator.keyboard.lock(['Escape'])`
  where it exists, and Alt+S always stops. Insert is the screen readers'
  modifier key, so `;` is the documented key.
- During a run, F1–F10 and F12 lose their browser defaults, through the F11
  pattern plus a document listener like the paddles' (`paddle.rs:459-475`).
- Every key has a button. The key bar's "Enter · TU & log" chip names what
  Enter sends, and F3 reads "F3 TU (no log)".
- Paddles key into the focused field (`paddle.rs:287-299`) and never
  transmit; phase 5 adds a field space after a 7-dit gap.

### 4.6 "Repeat their call on demand"

Again sends `?` on the air, exactly as MRCE's F7 (`Contest.pas:478`). The
station sends again with the same voice and fresh fading.

- Before your exchange: every caller still calling repeats. To ask only
  some, type a partial (`K5X?`) and press Enter (4.4).
- After your exchange: the station you work sends its exchange again.

Each repeat you ask for adds one send to the items it re-sends, and Dust's
repeat factor, `0.5^(sends − 1)` (`score.rs:111-112`), halves their points,
as group repeats already score (RufzXP's F6 penalty); its air time also
lowers the rate. A free local replay would teach the wrong reflex. An
exchange the station you work sends again costs its send whatever key drew
it: `?`, `NR?`, your exchange again (F2) or its call again (F5). A single
caller's call brought again by CQ or TU, once it had begun to reach you,
is a `?` by another key. Re-sends you did not ask for (timeouts, calling
twice, corrections, pile-up calls before you picked the station, a send a
hidden page or a failed sound cut short and sent again after Resume) cost
only time (9.1).

### 4.7 Partials, AGN, wrong calls, dupes

- **Partial calls.** `?` stands for one character, a trailing `?` for any
  tail, matched unanchored as MRCE's regex (`DxOper.pas:546-590`). Without
  `?`, edit distance decides: 0 is exact; up to (length − 1)/2, or the
  pattern inside the call, is close (`DxOper.pas:597-631`). A close piece
  that is no call itself (`K5`, `5XYZ`) asks for the rest as a partial
  does, and is not put right as a slip is; a call with no character but
  `?` is the `?` of F7, nobody's call. Only the best share of matched
  characters answers; the single caller always does.
- **AGN, NR?** ask for the exchange while working someone, and for calls
  otherwise. Typed in the Call field, `AGN` is just text.
- **Giving up.** CQ while working a station ends that QSO (4.3), and Enter
  starts the next afresh (4.4). Wipe clears the fields and resets C and N
  (`Main.pas:1984-1998`).
- **Dupes** (phase 2). After five QSOs, 2% of arrivals are stations already
  worked (11.6). A typed call already logged clean shows "B4 07:31" and the
  B4 button. A dupe scores 0 and is not an error, as contest rules remove
  dupes without penalty; MRCE counts it in Raw (`Log.pas:654-660`), so
  Dust's raw score is lower on runs with dupes. Before phase 2, a call
  already in the log or on frequency is drawn again, up to 8 times.

### 4.8 Logging and the check

A QSO is logged on ESM's TU row, on `.` `,` `+`, or on Shift+Enter (V and
E Complete). The machine records the times, the logged and true values, the
caller's speed and sends, and your serial; the fields clear and the serial
goes up. A logged QSO is final. Its check is shown when the station is done
(`Contest.pas:855-895`): once the turn carrying your TU is off the air,
however it leaves, ended, cut by Esc or Pause, replaced by your next key, or
at the run's end. A key while the TU still sounds sends it again (5.2), and
the check waits for that turn. Until then the row shows `…`. A QSO logged
without sending is checked at once, so nothing is `…` at the end, after
what of the turn on the air has reached you counts. It is checked against
the station you work when the call logged is its own or close to it (4.7),
or else against the station your TU without logging (F3) closed, as MRCE's
log looks for a station done with you (`Log.pas:745-755`); otherwise it is
a NIL. That QSO waits to be logged until another is, or you work another
station, and is lost then. The checks follow MRCE's `Log.pas:1030-1100`:

- **NIL:** nobody matching was worked. It counts raw, not verified.
- **CALL:** a busted call, with the correct one shown.
- **DUPE:** already logged clean.
- **Field errors:** NR, ZN, ST, PWR, CL, SEC, NAME, and RST in Simple QSO.

`QsoCheck` holds the errors found, a NIL alone or a busted call and field
errors together, and whether the QSO is a dupe, which is no error: a dupe
copied right is clean. Raw drops a call already in the log as typed, as a
logger marks a dupe; verified counts each clean, non-dupe station once.

Logged QSOs cannot be edited, as in MRCE.

## 5. Audio

### 5.1 A turn is one send

The players hold one send; starting another releases it
(`mixer/mod.rs:100-106`, `web/mod.rs:229-230`), and desktop UI timers fire
too late for a 0.1–0.6 s reply gap (`paddle.rs:94-96`). So each of your
actions is one send, a turn, holding your message and every reply:

```
time → |== you: K5XYZ 5NN TT7 ==|.. 0.1–0.6 s ..|== K5XYZ: TU 5NN 214 ==|
                                                   |=== 2nd caller ===|  (pile-up)
```

Your message is fixed when you key, so every reply can be decided then. A
pure `plan_turn` builds the plan. `wanted` is your message, with
`duration_sec` stretched to the last reply's end; both players play to
`wanted`'s length (native `SendVoice` ends there plus its tail,
`mixer/send.rs:150-170`). `others` are the replies, delayed and never cut,
so `plan_transmission`, which cuts at `wanted`'s end (`plan.rs:52-66`), is
not used. A turn of callers only has a `wanted` with no tones. Only
patience, the countdown, the resume timer and the run end use UI timers,
all seconds long. The app gains `start_planned` beside `start_send`
(`audio/mod.rs:81-90`) and `Lease::send_planned` beside `Lease::send`
(`player.rs:289-314`), keeping leases, stall
detection and retries (§12). What the turn says goes to the player as
`Turn::transcript()`, a `Transmission` of your message with each reply as a
station calling across it after its delay: the real players play only the
plan, and the test recorder keeps both, with the `Qsb` the send was given.

### 5.2 Continuations, stops, failures, heard time

- **A key during a turn** does not cut your message, as MRCE queues a
  message keyed while you send (`MyStn.pas:119-136, 177-187`). The machine
  plans a continuation, a new turn: the rest of your message from the
  character sounding at the key, sent again whole; then the new message;
  then the replies to it, which the callers decide on the new message alone,
  having heard the rest when it was keyed (4.3). Starting it releases the
  old send at once, while the new one starts a player's lead later, so the
  rest starts from the key, not from when the new send can start; and
  `heard_from` includes the lead and latency, so a character is sent again
  rather than lost. In a pile-up, callers still sounding carry on from their
  character at that moment, re-based, so collisions stay audible; a single
  caller stops and hears you.
- **Only Esc cuts.** StopTx emits `StopAudio` (an 8 ms release,
  `audio/shape.rs:13`); a call or exchange not fully sent counts as not
  sent, as MRCE's Esc resets CallSent and NrSent (`Main.pas:884-891`). The
  callers heard the message when you keyed it (4.3), so Esc takes back what
  had not gone out whole: they are as they were before the message keyed
  last, then hear only its parts sent whole. A reply it cut never sounded,
  and what they owe comes on their timeouts. A QSO the message itself lost
  (a CQ or TU keyed by mistake) is taken back with it, and so is a caller
  it brought, as MRCE's callers hear nothing of an aborted send
  (`AbortSend`); the groups of such a QSO reach the sampler only once it
  is final. A QSO logged since that key, or lost by a later key, is final,
  so then nothing is taken back; nor is a continuation's carried rest,
  heard at its own key. After Wipe, or a key with the Call field empty, C
  and N stay reset.
- **Stale sends.** A stopped or replaced send returns `Cancelled`
  (`player.rs:325-349`): `TurnCancelled`, ignored. Arming cancels the send,
  not the lease (`audio/epoch.rs:6-9, 21-27`; `player.rs:236-238`). Each
  turn's send is asked for as the turn goes on the air
  (`Lease::ask_to_send`), not when its task first runs: that ends every
  send asked for before it, so an older turn waiting out a retry gives up
  rather than start over the new one, and an Esc handled before the task
  runs still stops it.
- **A failure.** After every retry fails, `TurnFailed` with the current id
  pauses the run ("Sound stopped — Resume to carry on"). The retries are not
  run time: each attempt waits the turn plus 2 s (`wait.rs:18, 95-96`;
  `SEND_ATTEMPTS` 3, `player.rs:28`). So the limit does not end the run
  while the turn on the air may be retrying, once an attempt after the
  first has started or the one playing is past the turn plus 2 s: its end,
  a key or End run still end it, its failure pauses it with the retries
  given back, and the run end waits till its last attempt would have
  failed.
- **Heard time.** `send_planned` calls `on_start` after each attempt starts,
  with no `Player` borrow held (`player.rs:194-212`). The runtime dispatches
  `TurnStarted{id, heard_from}`: the start plus the player's lead and
  latency estimate (web `schedule_lead_sec` and `heard_latency`,
  `wait.rs:124-132`, `web/mod.rs:114`; native `latency_frames`,
  `audio/native.rs:272-280`). A
  caller's segment counts as heard once it has reached the ear; your own
  count from the start, which is lenient. Each player's send says it as
  `PlaybackSignal::heard_after_sec`: the browser's lead and believed
  latency, read as the send is scheduled; natively, the latency the mixer's
  last callback found; nothing for the silent player and the recorder.

### 5.3 Your own transmission

You hear your messages from phase 1, at the side-tone centre
(`settings/ranges.rs:175-178`) and your run speed (6.6), with no Farnsworth
spacing and no fist variation.

- **Slice 1a** plays it as `wanted` through the receiver at `VOLUME_MAX`.
  At the hardest noise it is only a few dB over the floor (2 dB SNR at full
  level, `band/calibration.rs:35-36`), and the AGC ducks a weak reply's
  first 0.3 s: an interim.
- **Slice 1b** gives it a dry path at a fixed monitor level, marked
  `dry_wanted` in the plan. The web player schedules it into
  `chain.floor_in`, past the filter and AGC but through the limiter
  (`ScheduledSend::schedule` takes an `into` node, `web/send.rs:21-27`);
  native sums an unfiltered `SendVoice` after `send * gain` in
  `Mixer::render` (`mixer/mod.rs:155-176`).

The turn plays from a copy of the run settings with `qsb_enabled = false`:
QSB is attached per send (`Qsb::of`, `audio/native.rs:241`) and `BandKey`
has no QSB field (`band/key.rs:19-49`), so the band never rebuilds. Callers
fade on their own from phase 2 (5.5).

### 5.4 Pile-ups against the filter

`resolve_pileup` keeps one wanted station loudest and nearest the centre
(`timing/pileup.rs:142-147, 338`). A contest has no wanted station, so
phase 2 adds `place_contest_callers`:

- **Pitch.** Single: the centre ±50 Hz. Pile-up: within ±`pileupSpreadHz`,
  at least 60 Hz apart (`pileup.rs:82`), inside the filter's −15 dB reach
  (`pileup.rs:158-174`).
- **Strength.** U-shaped across the volume range, as MRCE draws it
  (`DxStn.pas:101`), never below `volume_min`. The band's guard keeps
  background 6 dB under `volume_min` only within the side-tone range ±150 Hz
  (`band/activity.rs:110-119, 356-357`), so callers placed wider or faded
  can meet band traffic at their level, as on a real band.
- **Capacity** is the reach width ÷ 60 Hz + 1, about six on a 300 Hz filter;
  the rest wait. The Band card says "Your filter fits 6 callers".

### 5.5 Per-caller QSB (phase 2)

The envelope curves carry the gain: `build_envelope_curve` bakes
`target_gain` in (`plan.rs:99-102`); the native player reads `target_gain`
only for a curve with fewer than two points (`mixer/send.rs:63-66`), the
web player only when the browser refuses a curve (`web/send.rs:130-139`).

Each caller gets the band's `Qsb` model at its own time offset, following
the Band card's QSB switch. `plan_turn` bakes the fade into each tone's
curve point by point at `ENVELOPE_SAMPLE_RATE` (`envelope.rs:9`), one `Arc`
per tone, and scales `target_gain` to match. The QSB control runs
0.03–1.5 Hz (`settings/mod.rs:47-48`) with components up to 2.3× that
(`band/qsb.rs:26`), so a fade held per tone would step audibly. The web
curve cache (`web/send.rs:94-109`) then holds a few hundred entries per
turn. No player changes. Flutter (3–33 Hz, `DxStn.pas:97-99`) waits for
phase 5.

### 5.6 QRM and the background

The band runs as now (`band/mod.rs:9-30`); its traffic already sends contest
phrases, more as `stationsMax` rises (`activity.rs:194-195, 1281-1305`), and
at one station it is empty (`activity.rs:43-50`). While callers sound, the
background gives up slots counted from `plan.others.len()`
(`mixer/mod.rs:150-154`; web `Reservation`), so your message takes none.
On-frequency QRM comes with Caller habits `rough` (phase 4): a scripted
station at 30–50 WPM sending `QRL?`, a long CQ or `<you> QSY QSY`
(`Contest.pas:783`, `QrmStn.pas`).

### 5.7 Native, web and silent

The silent player completes a turn at its planned length; the app's tests
use the fake recorder (`src/testing.rs:131-133`). On a hidden page the web
clock parks and a send waits (`wait.rs:88-107`); the run pauses first.

### 5.8 Later

Phase 5 adds a receiver gate after the AGC while you send, as a non-QSK rig
mutes (`Contest.pas:806-834`); per-station gain for flutter; and a player
that starts a send at a given time, so a continuation splices in a gap.

## 6. Clock, pause and counters

### 6.1 Run length

`contestMinutes`: default 10, range 0–60; 0 means "until I stop", still
ending at 60. MRCE allows 1–240 (`Main.dfm:877-885`); users share
10-minute runs.

### 6.2 Start

Start contest opens the training screen. The shared countdown runs in the
log's place, fields disabled; then the clock starts and your first CQ goes
out by itself.

### 6.3 The clock

`RunClock` (cw-core) holds `started`, `paused_ms` and `paused_since` in
`MonoMs` (`session.rs:54-67`), never wall time, and the latest time it was
read, paused or resumed at. A pause stamped in the past (the heartbeat's)
begins no earlier than that, so it never counts a stretch of time twice nor
takes back run time a QSO was stamped with; a stale reading shows the
latest. Only a reading whose run time is kept (a key, a turn's end, a QSO,
the run's end) counts; checking the limit on any other event is a look
(`peek_ms`), so a forgotten timer waking mid-turn cannot keep a failed
send's retries as run time, nor can the run end, a stale send's news or a
forgotten timer end the run while the send may be retrying (5.2). Pausing
twice or resuming a running clock changes nothing. The machine has no
tick; the run bar redraws every 250 ms from `mono_ms()`, kept apart from
the view as `TrainingProgress` is (`training.rs:19-45`). It always shows
elapsed time: "07:42 / 10:00", or "07:42 · up to 60:00" for an open run.

### 6.4 Pause, hidden page, locked phone

- **Pause** (button or Alt+P) stops the turn, forgets every timer, holds the
  clock and releases the audio (`player.rs:111-117`), so the band falls
  silent and the screen may sleep. Callers and the open QSO wait; the fields
  lock, so a pause gives no free thinking time, and a copy typed before it
  and sent after it is timed in run time, the pause left out (9.1). A send
  the pause cut after it began to reach you is owed on its caller's
  timeout, as one heard whole, so a pause is no free `?`; one cut before
  it began goes again a moment after Resume (4.2).
- **Resume** claims a new lease inside the tap, as Start does
  (`session_runtime.rs:204`), so the player opens, or after a sound failure
  is rebuilt, within the browser's gesture: a player that failed every
  attempt is thrown away, so that claim opens a fresh one. Every contest
  key and button first calls `Lease::wake()`, which resumes a suspended
  context and opens a missing player (`player.rs:142-148`).
- **A hidden page or a locked phone** pauses the run, because `mono_ms`
  keeps running while hidden (`src/time.rs:29-50`). Three signals feed it:
  - a `visibilitychange` listener that talks back through `dioxus.send` and
    a `recv` loop, as `file_pick.rs:76, 113-114`, removed at run end;
  - on desktop, where wry never hides a minimised window, a
    `use_wry_event_handler` (as `paddle.rs:422`) checking
    `window.is_minimized()` on Resized and Focused;
  - a heartbeat: the run bar's ticker stamps `last_tick`; before any event,
    a gap over 2 s first dispatches `Pause{Hidden, at: last_tick}`, so a
    process frozen before any message arrived is still credited. The check
    starts with the ticker's first tick, so nothing is read into the moment
    before the run screen is up.
- **Never resumes by itself:** "Paused while Dust was in the background".
  The panel offers Resume, End run and Discard, which asks once more
  ("Discard this run? Its QSOs are not kept").

### 6.5 Time up

At the limit the run ends at once. A QSO is logged when its TU is keyed,
so one whose TU was still going out is in the log and checked; any other
open QSO is not logged, as in a real log, and counts as a lost copy (9.1).
MRCE's 15 s drain (`Contest.pas:184-220`) exists only to finish a TU in
flight, which Dust's "a logged QSO is final" rule covers.

### 6.6 Counters and speed

- **QSOs:** logged, dupes and NILs included as in a real log, with "n
  clean" beside it; a dupe copied right is clean.
- **Rate:** QSOs per hour over the last 5 minutes of run time, as MRCE's
  `ShowRate` (`Log.pas:1133-1147`), shown after the first minute; Simple QSO
  shows minutes per QSO.
- **Speed.** Callers' speeds are drawn from the Speed card's ranges plus the
  run offset, through `resolve_station` (`timing/station.rs:33-47`). You
  send at the middle of the shifted character range. − / + moves the offset
  by 2 WPM, MRCE's step (`Ini.pas:238`), within the Speed card's bounds, for
  your next message and callers who arrive later; callers already calling
  keep their speed (`DxOper.pas:235-262`). The offset lasts for the run and
  is not saved.

## 7. Screen layout

The run screen is `TrainingRoute` → `div.stack.train`, with the same cards,
keypad and action row in one column (`.shell`, max 760 px,
`styles.css:328-333`). It reads "a bit different" because of what is in it:
a running clock where "n / total" was, two labelled fields where the answer
box was, an air line and message buttons, and a log where the previous
group was. `ProgressHeader` (`widgets.rs:341-368`) gains `figure` and `pct`
in place of `current/total`.

**Desktop, running.** No keypad without a touch screen
(`training.rs:393-397`).

```
┌──────────────────────────────────────────────────────────────────┐
│ ·− Dust  CW group trainer                        ◐      [ Exit ] │
├──────────────────────────────────────────────────────────────────┤
│ ● On air · CQ WPX as DL1ABC                     07:42 / 10:00    │
│ ████████████████████████████████████░░░░░░░░░░░░                 │
│ QSOs 9 · 8 clean    Rate 70/h                   WPM [−] 24 [+]   │
├──────────────────────────────────────────────────────────────────┤
│ Copy their number into Exchange, then TU & log.                  │
│ TX ▸ K5XYZ 5NN T1T                               RX ▮ 1 calling  │
│ ┌ CALL ────────────────────┐  ┌ EXCHANGE · NR ────────────────┐  │
│ │ K5XYZ▌                   │  │ 214                           │  │
│ └──────────────────────────┘  └───────────────────────────────┘  │
│ Reads as: NR 214                                                 │
│ [Enter · TU & log] [F1 CQ] [F2 Exch] [F3 TU (no log)] [F5 His]   │
│ [F7 ?] [Esc Stop]                                                │
├──────────────────────────────────────────────────────────────────┤
│ Log                                              9 QSOs · 8 clean│
│  9  07:31  W6XYZ     214        24 WPM          clean            │
│  8  06:58  K5ABD     76     ?×1 busted call, was K5ABC           │
├──────────────────────────────────────────────────────────────────┤
│ [ ⏸ Pause ]                          [ End run ]   [ Discard ]   │
└──────────────────────────────────────────────────────────────────┘
```

- **The hint line** is `esm()`'s `hint` ("Type the call you hear, then
  Enter"), one line, as group training's (`training.rs:566-585`).
- **The air line** shows only your own message, the part still being sent.
  RX counts the stations sending but never shows what they send (Show copy,
  phase 4, is the exception).
- **A correction** turns the Call border to `--warn`: "They corrected you:
  fix the call, then Enter."
- **The log** is newest first, six rows tall. A row shows `…` until its
  station is done, then a word ("clean", "busted call, was K5ABC"), since
  icons are `aria-hidden` (`widgets.rs:277`). `?×1` counts asked repeats.

**Phone, running** (keypad always shown). From header to keypad it fits one
screen (`styles.css:2139-2175`); below 760 px height the keys shrink. Every
line is a fixed one-line slot with an ellipsis (as `.train-hint`,
`styles.css:1214-1224`), so the keypad never moves between turns.

```
┌────────────────────────────────────┐
│ ·− Dust              ◐    [ Exit ] │
├────────────────────────────────────┤
│ ● 07:42 / 10:00   9 QSOs   70/h    │
│ ███████████████████░░░░░░░         │
│ CQ WPX            WPM [−] 24 [+]   │
├────────────────────────────────────┤
│ Copy their number, then TU & log.  │
│ TX ▸ K5XYZ 5NN T1T      RX ▮       │
│ CALL            EXCH · NR          │
│ [K5XYZ▌     ]  [214         ]      │
│ Reads as NR 214                    │
│ [ CQ ]   [ ? Again ]   [ Wipe ]    │
├────────────────────────────────────┤
│  1  2  3  4  5  6  7  8  9  0      │
│  Q  W  E  R  T  Y  U  I  O  P      │
│  A  S  D  F  G  H  J  K  L  /      │
│  Z  X  C  V  B  N  M  ?    ⌫       │
│ [  ␣ Field  ]  [   TU & log  ↵  ]  │
├────────────────────────────────────┤
│ [ ⏸ Pause ]        [ End run ]     │
└────────────────────────────────────┘
   Log (scroll) · [ Discard ]
```

The keypad takes its rows as props, not the constants at
`training.rs:353-357`: the character
rows stay as in group training, only `,` and `.` leave row four, and the
last row is `␣ Field` (Call to Exchange; a space in Exchange) and the ESM
key, whose label and `aria-label` name what it sends. Fields keep
`inputmode="none"` (`training.rs:205`); a tapped field takes focus and the
paddles.

**Typing never loses keys.** A controlled input redrawn while typed in drops
the key typed in between (`training.rs:47-60`). Each field is an
`AnswerBox`-style component over `ContestEntry` (§12), redrawn only for
keypad or paddle letters, Wipe and clearing. The hint, the Enter label and
"Reads as" are sibling components reading a per-key signal. Upper case comes
from CSS; text is normalised on read, never written back.

**Accessibility.** The clock is `role="timer"` with live updates off. One
visually hidden polite region speaks discrete events only (a QSO logged and
its check, a correction, paused, resumed, time's up), queued until the air
is silent so speech never covers the Morse.

**Paused,** the entry card gives way to "⏸ Paused. Your QSO waits: W7XYZ ·
37", a full-width Resume and the log, with Discard and End run below.

**Results** (a contest branch of `ResultsView`, `src/ui/results.rs:59-167`;
the same view opens read-only from a History row):

```
┌────────────────────────────────────────────────┐
│  ╭─────╮   Run complete · CQ WPX · 10:00       │
│  │ 92% │   12 QSOs, 72 an hour. Two calls      │
│  │CLEAN│   needed a repeat; one NR was wrong.  │
│  ╰─────╯                                       │
├────────────────────────────────────────────────┤
│ (3) 20.7 points a group · 3rd of your 14 WPX   │
│     runs                                       │
├───────────┬───────────┬───────────┬────────────┤
│ QSOS 11/12│ RATE 72/h │ ACCURACY  │ POINTS 496 │
│           │           │ 97%       │            │
├───────────┴───────────┴───────────┴────────────┤
│ Contest score     QSO pts · Mults · Score      │
│ Raw                    12 · 12 · 144           │
│ Verified               11 · 11 · 121           │
│ 0 busted calls · 1 wrong NR · 0 NIL · 0 dupes  │
├────────────────────────────────────────────────┤
│ Log, with what was really sent                 │
│ 2 01:20 DL2XYZ 13→31  26 WPM  ?×1  wrong NR    │
├────────────────────────────────────────────────┤
│ [ ↻ Run again ]                 [ ← Back ]     │
└────────────────────────────────────────────────┘
```

The ring, captioned "QSOs clean", is the share logged clean; ACCURACY is
character accuracy, as Home and History show it; POINTS are Dust's copy
points (10); the banner ranks within the contest kind. Simple QSO shows
QSOs, minutes per QSO and accuracy per field, with no contest score. A
History row reads "2026-10-20 · Contest · CQ WPX · 11/12 QSOs · 72/h · 97% ·
496 pts".

**Settings, Contest chosen.** The Character set card (§2) shows the six
segments in a 3 + 3 grid, the contest pills, "You send K5XYZ 5NN TT1 · You
copy NR", the dialogue (You `CQ DL1ABC TEST` · They `K5XYZ` · You
`K5XYZ 5NN TT1` · They `TU 5NN 214` · You `TU`), the tier stepper and the
run style. Your station follows it.

## 8. Settings

Settings stay flat camelCase sections (`settings/mod.rs:551-564`). The user
asked that settings be shared, so the design reuses as many as it can.

**Shared unchanged:** keying envelope; tone and volume (the pitch centre is
the filter centre and your pitch; the volume range is the callers'); band
conditions; character sampling, which steers the callers' calls
(`sampling.rs:187-195`); the countdown and keyer trio; extra word space and
fist variation.

**Reinterpreted.**

| Setting | In Contest |
|---|---|
| Character and effective speed ranges | the callers' speeds; you send at the middle; −/+ shifts both for the run |
| `callsignLevel` | the callers' call shapes, shared with Callsigns mode |
| `stationsMin/Max` (`settings/mod.rs:414-417`) | Single: labelled "Band activity". Pile-up: "Callers at once", as today |
| `pileupSpreadHz` | Pile-up only: how far either side of the centre callers land; hidden for Single, where it does nothing |
| Session shape's first field | "Minutes per run" in place of "Groups per session" (`settings.rs:288-295`) |

**Hidden in Contest mode** (`app.rs:1606-1635`): Experience presets; the
group fields (groups per session, group size, adaptive size, Aim, sends,
answer timeout, pause after a group, lock typing); Pile-up level, which
needs a wanted station; Adapt speed to me, until phase 5; Auto level; the
alphabet controls.

**Presets** (phase 2): First QSO, Club and Contester on the contest card.
Each writes shared fields (speed ranges, tier, stations, QSB, QRN, later
Caller habits) and reads "Custom" once any is edited, as Experience does
(`experience.rs:83, 98-110`, `settings.rs:135-177`). No level is stored.

**New fields.** One flattened struct, `ContestSettings`
(`settings/contest.rs`), joins `TrainingSettings`, `SettingsSave` and its
`From` (`settings/save.rs:17-31, 61-96`). A new `SettingsSection::Station`
joins `ALL` (`settings/mod.rs:493-525`) and `reset_section`
(`settings/reset.rs:11-100`).

| Serde name | Type | Default | Clamp | Section | Phase |
|---|---|---|---|---|---|
| `contestKind` | `sprint` `arrlDx` `wpx` `cqww` `fieldDay` `simpleQso` | `wpx` | unknown → `wpx`, forgiving as `FilterShape` (`settings/mod.rs:101-114`) | Character set | 1 |
| `contestMinutes` | u32 | 10 | 0..=60 (0 = open, ends at 60) | Session shape | 1 |
| `myCall` | String | `""` | upper case; `A–Z 0–9 /`; no leading, trailing or doubled `/`; ≤ 12 | Station | 1 |
| `myName` | String | `""` | `A–Z`, ≤ 10 | Station | 1c |
| `contestStyle` | `single` `pileup` | `single` | forgiving | Character set | 2 |
| `myState` | String | `""` | a state, DC, province, territory, `LB` or `DX`; else `""` | Station | 2 |
| `myZone` | u32 | 0 | 0..=40 (0 = from your call) | Station | 3 |
| `myClass` | String | `1D` | `[1-9][0-9]?[A-F]`, else `1D` | Station | 3 |
| `mySection` | String | `""` | a listed section, `YT`, or `DX`; else `""` | Station | 3 |
| `myPower` | String | `100` | 1..=1500, `K`, `KW` or `1K`; else `100` | Station | 3 |
| `contestHabits` | `easy` `real` `rough` | `easy` | forgiving | Band conditions | 4 |
| `contestShowCopy` | bool | false | — | Session shape | 4 |

- **Caller habits.** `easy`: plain digits, no LIDs. `real`: numbers as
  MRCE's callers send them (11.6). `rough`: LIDs and on-frequency QRM.
- **On disk, Contest is written like a session (9.1):** `"charSetMode":
  "callsign"` plus `"contestActive": true`, a key that exists only on disk
  and that `From<SettingsSave>` reads back as Contest. An older build opens
  in Callsigns at the same tier instead of failing the strict parse into
  Mixed (`settings/save.rs:44-57`, `persist/mod.rs:183-207`). Its next save
  drops Your station: settings, which a backup holds, not training data.
  Nothing writes `"contest"` as the mode; a save edited by hand that names
  it opens in Contest, and `start_session` starts a run from it.
- **Clamping and tests.** `clamp()` settles the strings in one pass
  (`clamp.rs:15-115`); every new number is an integer. `wild()` and
  `wildly_different()` (`settings/mod.rs:826, 1382-1441`) set every new
  field, for the clamp tests and for
  `every_setting_belongs_to_exactly_one_section` respectively. Nothing joins
  `PROGRESS`.
- **Starting** needs a `myCall` that `parse_callsign` reads and valid
  contest fields; Home and the card say what is missing, in the words of
  `ContestSettings::missing()`. Until they do, a start refused says it in a
  message.

## 9. Data model and storage

### 9.1 SessionResult

A run is one `SessionResult` in the one history. A separate store would hide
contest practice from every stat, because every stat reads
`&[SessionResult]`.

```rust
// session.rs
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", from = "StoredSession")]
pub struct SessionResult {
    // ...every field as today (session.rs:589-618)...
    /// Contest is written "callsign", so a build that knows no contests
    /// still reads the session.
    #[serde(serialize_with = "mode_on_disk")]
    pub char_set_mode: CharSetMode,
    /// "contest:wpx", or "contest:wpx:a" when assisted: survives a lost log.
    pub alphabet_fingerprint: String,
    /// The run's log exactly as written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contest: Option<ContestLog>,
}

/// `serde_json::Value`, written back as read. `run()` parses it on demand.
pub struct ContestLog(pub serde_json::Value);
impl ContestLog { pub fn run(&self, groups: usize) -> Option<ContestRun>; }
```

**Reading.** `Deserialize` goes through a private mirror, `StoredSession`,
whose `From` sets `char_set_mode = Contest` when a log is present or the
fingerprint starts with `contest:`, so every read path (`parse_sessions`,
`persist/mod.rs:160-175`; `backup.rs:88-95`) gets it. `serde_json` becomes a
cw-core dependency. `GroupSession::result` maps Contest to Callsign, with a
`debug_assert`, so no group session is recorded as Contest.

**`run()`** parses a strict `ContestRun` (no container default, no catch-all)
only when `v` is exactly 1, the kind, style and field names are known to
this build, and the `g` values add up to `groups`. Otherwise the log stays
as read, shown as "Contest run (this version cannot read its log)", or
"cannot score Sprint" for a known layout with an unknown kind. History
parses each log once per load.

```rust
// contest/record.rs
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContestRun {
    pub v: u32,                   // layout, 1
    pub kind: String,
    pub style: String,
    pub callers: [u32; 2],        // the stations range used
    pub side: String,             // "wve", "dx", or "" where sides do not apply
    pub my_country: String,       // facts the score uses, stored because
    pub my_continent: String,     //   the tables that derive them change
    pub my_zone: u32,
    pub fields: Vec<String>,      // each QSO's copied fields: ["nr"]
    pub limit_s: u32,
    pub run_ms: u64,              // pauses left out
    pub paused_ms: u64,
    pub my_wpm: f64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub assisted: bool,           // Show copy was on (phase 4)
    pub qsos: Vec<QsoRecord>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QsoRecord {
    pub t: u64,         // logged or lost, ms on the run clock
    pub h: u64,         // first heard
    pub g: u32,         // groups it owns
    pub nr: u32,        // your serial; 0 when the contest has none
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub k: String,      // "" logged, "nil", or "lost"
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub c: String,      // the station's country key, for multipliers
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub l: Vec<String>, // the exchange you logged, a token a field, where no group holds it
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub lc: String,     // the call logged, where the call group does not hold it
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub s: Vec<String>, // what the station sent of it, beside `l`, for the check
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sc: String,     // the station's call, where an earlier QSO holds its copy
}
```

Your callsign and exchange are not stored in a run: no score needs them.
Checks, dupes, points, multipliers and rates are worked out on read, as
`score` is. `v` rises only for a change an older reader would misread; a
key that later phases add with a default does not raise it.

**How a run fills the groups** (`ContestRun::result()`, which takes the
groups the machine kept, as the stored layout holds none;
`ContestMachine::result()` builds both). QSO *i* owns the next `g` groups.
`ContestRun::score(groups)` works the contest score out again from them,
exactly as it was while the run went on.

- **A logged QSO:** one group for the call, then one per copied field
  (RST only in Simple QSO), so no false bigram crosses a call and its
  exchange in Mistakes. A field is a copy only once the station's exchange
  has begun to reach you: logged before that, each field group is a miss
  (received empty, 1 send), as a guess is no copy, so logging early neither
  raises accuracy nor shrinks the groups a run is ranked over; `l` and `s`
  hold what you logged and what was sent, for the check.
- **One station, one copy.** A station logged again (a dupe, or a log
  after a NIL charged to it) owns no groups: `sc` holds its call, `lc` the
  call logged, and `l` and `s` both exchanges. The soak checks that no
  station's groups are owned twice.
- **A lost QSO** (`k: "lost"`): a station whose call you sent, or that
  answered your partial, whose QSO ended unlogged (wiped, gone, failed by
  your CQ, closed by your F3 and never logged, open at the end). One call
  group; received = the call as first sent with your exchange, as a logged
  QSO keeps it, so walking away pays no better than finishing; never sent
  so, the Call field as last read while you worked it (a wipe reads it
  before clearing), without `?`; empty is a full miss. It is timed as the
  call was when you last sent it.
- **A NIL** (`k: "nil"`): one call group against the station you work, or
  else the station heard whose call the logged one is or is close to, or
  else the single caller, the newest heard, and never an older one;
  received = the logged call. That station then has no lost group as well.
  When that station already owns its groups (lost, logged, or charged a NIL
  before), or no station was heard at all, there is no group, and `lc`
  holds the call; no other station is charged.
- **Calls.** received = the call as first sent with your exchange, so a slip
  the station corrected still counts; your exchange sent with no call in
  the message (F2) goes with the call last sent to that station, whatever
  an empty Call field or Wipe did to C. The log keeps the corrected call,
  in `lc`, written only when it differs.
- **Fields hold what was keyed.** sent = the token transmitted (`1N5`,
  `TT7`, `5NN`). If the canonical values match (11.6), received = sent, so a
  logger-style `7` for `TT7` is clean. Otherwise received is the typed
  token read as values, a typed cut letter as its digit and leading zeros
  beyond the sent token's padding dropped, so `A5` for `14` is one error,
  not two, and `008` for `7` is one, not three. Each typed digit is
  written as the sent character where it matches and, where it does not,
  as the cut letter where the sent token had one (0↔T/O, 1↔A, 5↔E, 9↔N): a
  heard N typed as 0 is N→T. Digits are lined up against the sent token by
  value, so a heard N typed as 9 is a match even beside a digit added or
  dropped. Empty is a miss.
- **`sends`** is 1 plus the repeats you drew while the item was owed: a
  call you asked for again (`?`, a partial or a piece of the call, `AGN`,
  `NR?`, or a CQ or TU that brings a single caller's call again), alone or
  as its answer putting yours right, before you sent it with your
  exchange; an exchange the station you work sent again at any key of
  yours (4.6). A repeat counts once it has begun to reach you, even if you
  key over the rest of it, and only once an earlier send of the same had
  begun to reach you: a `?` keyed before anything of the station reached
  you asks for nothing again, and one keyed while its first exchange still
  sounds is a repeat. One Esc stops before it begins was never heard; one
  a pause or failed sound cuts before it begins is charged when it comes
  after Resume. A repeat counts only for what it could teach: a field's
  for fields empty, or read differently, after it; a call's when the Call
  field, without `?`, read otherwise at the key than the call finally
  copied.
- **`time_to_complete_ms`.** A call: from the end of its last send heard
  whole before the Call field's last edit, among the first and those a key
  of yours drew (CQ, TU, `?`, a partial), to that edit, at the key that
  sent it with your exchange; a call never sent so is timed as when you
  last sent it. Fields: likewise from the end of the last exchange heard
  whole before the field first read complete, among the first and those a
  key of yours drew, shared by length; a repeat drawn after the field read
  complete changes nothing. A send on a caller's timeout restarts neither:
  waiting costs time, not sends. `Fields` carries both stamps; the 100 ms a
  character floor applies (`session.rs:26, 177`). Both ends are in run
  time, so a pause between the edit and the key neither shortens nor
  lengthens a copy.
- **Speeds.** Each group carries the caller's speeds to 0.1 WPM; the
  session's are their mean, as `GroupSession::result` does
  (`session.rs:478-491`), never `my_wpm`. `digits_level` comes from the
  settings, `level` is the callsign tier, `score` is `points()`, and the
  tally and accuracy are worked out as for groups (`session.rs:458-521`).

### 9.2 What older builds do

| Build | Reads a run as | On its next save | Its stats |
|---|---|---|---|
| Before phase 0 | a Callsign session | drops the log; groups and the mark stay (`desktop.rs:215-228`) | Callsign copy speed, Callsigns adaptive speed (`sampling.rs:93`, `speed.rs:289-310`, blind to `sends`) and the Callsign board include runs; auto level untouched |
| Phase 0 | a contest run by its mark, with no log view | keeps the log as written | Contest pill and labels |
| Phase 1 and later | a run, scored when `run()` reads it | keeps the log as written | full |

Writing `"contest"` would make every older build drop the session in
`parse_sessions` (`persist/mod.rs:163-170`) and erase it on its next save.
After an upgrade, a stripped run is recognised by its mark: it leaves the
Callsign series and board ("log not kept by an older version"), so the
Callsign speed estimate and board places can shift; the release notes and
`docs/platforms.md` say so. Phase 0 ships a release before phase 1, so
fewer old builds meet a run.

### 9.3 Backups and merge

The layout stays `{dustBackup: 1, savedAt, settings, sessions}`
(`backup.rs:14-59`). Contest data rides inside sessions and settings.

From phase 1, `merge_sessions` (`backup.rs:114-132`) still matches on
`timestamp` and `date`, and a restored copy with a contest log replaces a
present one without. It returns `(merged, added, filled)`; the restore
message (`state.rs:319-326`) says "Restored 12 sessions and filled in 3".

### 9.4 Size budget

Web history is held to 3,000,000 characters (`persist/web.rs:25`), filled
newest first (`persist/mod.rs:231-248`); every store keeps at most 500
sessions (`persist/mod.rs:30`). Measured on sample sessions (compact JSON,
speeds to 0.1 WPM):

| Session | Characters |
|---|---|
| Group session, 20 groups | 3,100 |
| WPX, 12 QSOs (10 min at 70/h) | 5,100 |
| WPX, 17 QSOs (10 min at 100/h) | 6,500 |
| WPX, 25 QSOs (10 min at 150/h) | 8,800 |
| Simple QSO, 12 QSOs | 6,500 |
| Sprint, 200 QSOs (60 min at 200/h) | 107,900 |

A WPX QSO adds about 290 characters to about 1,600 fixed, so a 10-minute run
costs 1.6 group sessions at 70/h and 2.9 at 150/h, and in a browser a heavy
contest user keeps fewer than 500 sessions. The squeeze notice
(`HistorySaved::Squeezed`) gains "Long contest runs take more room", and
"The newest 500 sessions are kept" (`src/ui/privacy.rs:24`) becomes "Up to
500 sessions are kept; in a browser, as many of the newest as fit". Groups
cannot shrink: older builds require `received`, `correct` and
`timeToCompleteMs` (`session.rs:544-568`).

### 9.5 Privacy

`PrivacyCard` (`src/ui/privacy.rs:12-24`) and `docs/privacy.html` gain:

> In Contest mode, Dust keeps your callsign and the exchange you send (name,
> state or province, CQ zone, class, section, power) in its settings on this
> device. Each contest run keeps only your country, continent and CQ zone,
> so its score can be worked out again. Both are in any backup you save.
> Dust never sends them anywhere. Clear Your station to remove them. The
> stations you work are generated by Dust; if one matches a real station,
> that is chance.

The policy's Settings and Practice history lists gain the same items.

## 10. Stats integration

The rule: no setting hides past training (commit ba8822d; "Every session
scored on this device, whatever it practised", `src/ui/stats.rs:305`;
`docs/platforms.md:296-300`).

A run is a `SessionResult`, so every stat that counts sessions counts it.
Where a stat narrows by mode, Contest gets a pill and stays under All;
`char_set_mode` is `Contest` in memory, so the existing filters keep runs
out of the Callsign series (`belief.rs:134-145`, `stats.rs:420-429`,
`board.rs:57-59, 106-117`).

| Stat | Contest runs |
|---|---|
| Calendar, streak, accuracy | Count; character accuracy covers calls and fields, lost calls included. A day's best reads "Best points" (`heatmap.rs:231`). |
| Letters (`stats.rs:136-198`) | Accuracy counts every keyed character. Contest timings stay out of `avg_ms` and the slow flag, which measure answers after a send; the note says so. |
| Mistakes, sampler beliefs | Count, on keyed tokens: K5XYC for K5XYZ is Z→C, `1T5` for `1N5` is N→T. |
| Sampling snapshot (`stats.rs:317-397`) | A Contest arm runs the contest's `CallSource` and `CallSpec`, for calls only. |
| Copy span, group-length forecast | Not contest measures, as for Callsigns (`ui/stats.rs:211-245`). |
| Copy speed (`speed.rs:285-310`) | A Contest pill and series, from call groups and numeric fields (NR, ZN) only, found by the log's layout; a stripped run uses the groups `parse_callsign` accepts. `adaptive_speed_path` is `None` in Contest until phase 5; the card reads "Callers' speeds are drawn evenly from your range, shifted by − / + during a run". |
| Board (`board.rs`) | A Contest pill with a chip per kind; `board_place` keys on the mark's kind. Ranked by points per group within a kind; a run qualifies when its log reads and it ran 5 minutes or more, to a limit of 5 minutes or more, with a clean QSO; a run whose log an older build dropped is unplaced. Under All, WPX runs sit about 30% under Callsigns per group, since fields are short; the note says so. Assisted runs are greyed, unplaced, and out of record and form. |
| History (`ui/stats_detail.rs:362-371`) | Every row names its mode (phase 0); a contest row opens its results; a stripped run says "log not kept by an older version". |
| Auto level | Untouched: the contest finish never calls `complete_session` (`auto_level.rs:285-308`). |
| Runs card (phase 4) | Every run per kind: date, length, QSOs, rate, both scores, error share, best verified per kind and length. |

**Two scores, two jobs.**

- **Dust points** measure copying: 10 × characters × speed × errors × time ×
  repeats, weighted by the alphabet's entropy (`score.rs:116-142`). From
  phase 1 they read "Points" where today they read "Score" (`results.rs:133`,
  `heatmap.rs:231`) or a bare number (`stats_detail.rs:370`, now "496 pts").
- **The contest score** measures operating. `contest_score()` works it out
  from the log: Raw, every logged non-dupe QSO's points × the multipliers as
  typed; Verified, clean QSOs only, as MRCE's (`Log.pas:650-680`); rates and
  the best 5 minutes. Its columns read "QSO pts · Mults · Score".

A busted call loses both its QSO's contest points and its group's copy
points; a correction adds no send, so one error is not punished twice. The
contest score depends on contest, length and location, so it never ranks on
the main board.

## 11. Data and generators

Never a call. Only counts and shares derived from real data are kept
(`callsign.rs:19-20`, `scripts/callsign-prefixes.py:14-15`). No
call-history file ships, and no real name is tied to a call. Tables are Rust
files written by scripts, each naming its source and year.

### 11.1 Callers' calls

- **The source.** `Sampler` implements `CallSource` (§12) with the
  `most_wanted` choice among 6 candidates that Callsigns mode uses
  (`sampling.rs:384-406`), so calls lean to your weak characters, at the
  shared `callsignLevel`.
- **The generator.** `generate_call(tier, spec, rng) -> GeneratedCall
  { text, country, digit, host, marker }` in `callsign.rs`. A `CallSpec`
  filters countries (`Any`, `WVe` = K and VE, `NotWVe`, `NorthAmerica` = W/VE
  plus Alaska, Hawaii, Puerto Rico and the US Virgin Islands, who call in
  Sprint and Field Day). A station abroad is in its host's country. A
  contest spec uses contest shares for portables (4%) and abroad calls (3%)
  in place of the tiers' 25% and 12% (`callsign.rs:44-47`). With the default
  spec it draws exactly as `generate_callsign` does, which a golden list
  pins.
- **Parsing.** `parse_callsign` (`callsign.rs:376-377`), with `CallsignParts`
  (`:358`) and `parse_base` (`:393`), loses `#[cfg(test)]`; `parse_base`
  stays a private helper. When both sides of a slash read as a call (hosts
  such as `VP2M`, `VK9X`, `3D2R` do), the shorter is the host or marker, as
  `ExtractPrefix` takes it, and at equal lengths the side with the longer
  suffix is the call. `country_of` reads a call's country, host first (cut
  as for WPX below): a table entry whose prefix and a digit it issues spell
  the call's whole prefix and digit, or whose prefix alone spells a host,
  ties going to the country with more calls (`R9` is Asiatic Russia). A digit
  the table has not seen still places a prefix only one country has (`KG3`
  is K); a prefix the table lacks, or a shared one with such a digit, is
  `None` (`C31`, `NL7`, `VP8`), never a country that only shares a letter.
  The generator labels each call with the country `country_of` reads off it,
  so the two agree. `wpx_prefix` reads its WPX prefix: up to the last digit
  (`9A1AA` → `9A1`); a digit marker replaces the digit (`W1XYZ/4` → `W4`);
  a host is the prefix, with 0 added if it has no digit (`PA/N8XYZ` →
  `PA0`), cut as MRCE's `ExtractPrefix` cuts one, to its last digit but
  never under two characters (`9A/DL1ABC` → `9A0`, `VP2M/G3XYZ` → `VP2`); a
  country after the slash counts as a host (`W1XYZ/KH6` → `KH6`); `/P /M
  /QRP`, and `/A /MM /AM`, are ignored.

### 11.2 Places and the ARRL DX side

- **US state by call area:** 1 CT ME MA NH RI VT · 2 NJ NY · 3 DE DC MD PA ·
  4 AL FL GA KY NC SC TN VA · 5 AR LA MS NM OK TX · 6 CA · 7 AZ ID MT NV OR
  UT WA WY · 8 MI OH WV · 9 IL IN WI · 0 CO IA KS MN MO ND NE SD · KL7 AK,
  KH6 HI. P(state | digit) is counted from the FCC's ULS data (a US
  Government work, counts only) in phase 3; until then, population shares.
- **Canada by digit, for every prefix** (VA, VB, VC, VE, VG, CF–CK, XJ–XO;
  the table holds VE, VA, VC, CK and VB, `prefixes.rs:203-209`): 1 NS, 2 QC, 3 ON, 4 MB, 5 SK, 6 AB, 7 BC, 8 NT,
  9 NB. VO1 NL; VO2 LB in ARRL DX, NL elsewhere; VY0 NU, VY1 YT, VY2 PE.
- **W/VE side.** Your country, after any host, is K or VE. KH, KL, KP, AH,
  AL, NH, NL, NP, WH, WL and WP prefixes are DX, as are CY0 and CY9.

### 11.3 CQ zones and continents

- **US by state.** Zone 3: WA OR CA NV ID UT AZ. Zone 5: ME NH VT MA RI CT
  NY NJ PA DE MD DC VA WV NC SC GA FL. Zone 4: the other contiguous states.
  AK 1, HI 31.
- **Canada.** BC 3; AB SK MB ON 4; NS NB PE NL 5; LB 2; QC 5 about 99%, 2
  about 1% (north of 50°); NU 2 about 90%, 1 about 10% (west of 102°W);
  YT, NT 1.
- **Elsewhere,** cty.dat's entity zone and continent, plus its CQ overrides
  such as `R8(17)`, which `callsign-prefixes.py:71-78` reads and throws away
  today; VK6 and VK8 are 29, other VK 30, entered by hand because that
  cty.dat has ITU overrides for them but no CQ ones. Asiatic Russia and China are
  simplified, and `myZone` overrides.

North America is hand-entered from the CQ zone list, because cty.dat gives
Canada zone 05 bar a few northern prefixes, VE3, VE7 and VE8 included
(VER20200405, `prefixes.rs:5`). A test pins it row by
row.

### 11.4 Sections

The 85 ARRL/RAC sections are the non-DX codes in MRCE's `FDGOTA.txt` (less
one stray `MD`), as of
RAC's 1 January 2023 changes (MAR became NB and NS, NT became TER, GTA
became GH). RAC's YT (1 July 2025) is not in ARRL contests: callers send
TER; the parser and `mySection` accept YT. Sources disagree on Yukon's code
(YT or YUK), so it is checked against ARRL's list before phase 3 ships. The
list is versioned data.

- 1 CT EMA ME NH RI VT WMA · 2 ENY NLI NNJ NNY SNJ WNY · 3 DE EPA MDC WPA
- 4 AL GA KY NC NFL PR SC SFL TN VA VI WCF · 5 AR LA MS NM NTX OK STX WTX
- 6 EB LAX ORG PAC SB SCV SDG SF SJV SV · 7 AK AZ EWA ID MT NV OR UT WWA WY
- 8 MI OH WV · 9 IL IN WI · 0 CO IA KS MN MO ND NE SD
- Canada AB BC GH MB NB NL NS ONE ONN ONS PE QC SK TER

A Field Day caller's state is drawn first, then its section from counted
P(section | state) (TX's three sections under TX; DC under MDC). AK, PAC,
PR and VI callers come from KL, KH and KP at their counted shares.
`mySection` defaults from your call: KL → AK; KH6/7 and KH0/2/8 → PAC;
KP3/4 → PR; KP2 → VI; CY0/CY9 → NS; DX outside W/VE.

### 11.5 Exchange values

- **Names.** About 150 short North American given names from the SSA
  baby-name data (public domain), weighted to births 1935–1975 and folded
  to the forms operators send (ROBERT → BOB); short lists per region written
  for Dust elsewhere. A name is kept per caller, never tied to a call. The
  16 names at `band/activity.rs:1152-1155` move here.
- **Field Day class,** from 13,709 rows of `FDGOTA.txt`: 1D 41.9%, 1E 14.0%,
  1B 13.1%, 2A 6.7%, 3A 6.7%, 4A 3.5%, 1A 2.9%, 5A 1.8%; 1C, 2D, 2F, 2B, 2E
  about 1% each; the rest 4%.
- **Power,** from 3,779 DX rows of `ARRLDXCW_USDX.txt`: 100 34.7%, KW 25.0%,
  500 12.8%, K 10.9%, 400 4.6%, 200 3.0%, 5 2.4%, others 6.6%.
- **Reports** (Simple QSO), Dust's estimate: 599 40%, 579 20%, 589 15%,
  569 10%, 559 8%, 449 4%, 339 3%.

### 11.6 Serials, cut numbers, 5NN

- **Callers' serials** are `1 + floor(final × (T0 + t) / L)`, never past
  `final`: `L` is the contest's length (48 h, 4 h), `T0` uniform in
  [0, L − the run's limit] (60 minutes for an open run), `t` run time,
  `final` capped at 9,999 so a serial stays within four digits. `final` is
  log-normal: WPX median 190, σ 1.45, fitted to F6FVY's histogram of last
  serials in MRCE (`CqWpx.pas:127-196`); Sprint median 80, σ 0.9, Dust's
  estimate. Those bins count logs, and a station you hear tends to have a
  big log, so `final` is drawn with weight ∝ final^0.5, Dust's estimate
  between logs and QSOs. A dupe sends its previous serial + 1 +
  Poisson(final × Δt / L).
- **Habits `easy`:** plain digits, no padding (`7`). **`real`,** as MRCE:
  padded to three with the leading zeros as `TT` (`Station.pas:493-494,
  536-537`), 98% of other zeros as T, 9 as N 97% of the time (`:538-539,
  564-565`), zones and power cut to T, A, N for 70% of callers
  (`:555-562`), and `ENN` in 5% of messages (`:527-528`).
- **Your numbers** are always cut, except your Sprint serial.

| Field | Canonical | Equal when |
|---|---|---|
| NR, ZN | integer, no leading zeros | numbers equal (`TT1` = 1, `05` = 5) |
| RST | three digits | equal (`5NN` = `ENN` = 599) |
| ST | code | equal after PQ → QC, NF → NL; LB → NL in Sprint only |
| SEC | code | equal; YT = TER |
| PWR | number or `KW` | `K` = `KW` = `1K` = 1000; `1TT` = 100 |
| CL | number and letter | `03A` = `3A` |
| NAME | A–Z | equal |

The cut letters read are T and O for 0, A for 1, E for 5 and N for 9. No
cut letter is read outside a numeric field (NR, ZN, RST, PWR). The check
and the log use canonical values; the groups keep keyed tokens (9.1).

### 11.7 Parsing what you type

- **Call:** `A–Z 0–9 / ?`, upper case, no spaces. `?` asks and is never
  logged.
- **Exchange:** `parse_exchange(rules, side, text) -> Reading` runs on every
  edit and returns the fields, the unused tokens, the unreadable ones (what
  "Reads as" shows as the fault) and E (4.4). A token no copied field can
  read, and not a report, is unreadable and makes E Invalid. Phase 1 uses
  one rule per field: WPX NR is the last token that reads as a number and is
  not exactly `5NN`, `ENN` or `599` (with only those, 599); Simple QSO RST is
  the last report-shaped token, NAME the last all-letter one. From Sprint,
  one deterministic parser serves all: each closed-list field (state,
  section, class, power) takes the last token from its list; each numeric
  field the last remaining number; free-text fields fill from what is left,
  right to left. A correction typed last wins.

| Contest | Typed | Reads as |
|---|---|---|
| WPX | `599` | NR 599 |
| WPX | `5NN 145` / `TT1` / `5NN TT1` | NR 145 / NR 1 / NR 1 |
| CQ WW | `A4` / `5NN 05` | ZN 14 / ZN 5 |
| Sprint | `145 BOB TN` | NR 145, NAME BOB, ST TN |
| Sprint | `145 AL AL` | NR 145, NAME AL, ST AL |
| Sprint | `145 BOB TN 146` | NR 146, NAME BOB, ST TN; 145 unused |
| ARRL DX | `1TT` / `KW` | PWR 100 / PWR KW |
| Field Day | `OR 3A` | CL 3A, SEC OR |
| Simple QSO | `57N BOB` | RST 579, NAME BOB |

The reading shows under the field, unused tokens greyed. The reading is
stored, not the raw text, so a later parser cannot rewrite an old log.

### 11.8 Sources and licences

The repository is GPL-3.0-or-later (`Cargo.toml:9`).

| Data | Source | Status |
|---|---|---|
| Call shapes | `callsign/prefixes.rs` | counts only |
| DX zones, continents | cty.dat (AD1C) | no licence found; facts only, credited; ask AD1C; fallback CQ's country list |
| NA zones, places, sections | CQ, USPS, FCC, ISO 3166-2:CA, ARRL/RAC | public facts |
| State by call area | FCC ULS | US Government work, counts only |
| FD class, power, section shares | MRCE's `FDGOTA.txt` (W1MI, SM7IUN, W7SST), `ARRLDXCW_USDX.txt` (VE2FK, SM7IUN, K3CT, WV4P) | in an MPL-2.0 repository, compiled from contest logs; rounded shares only; credited; ask W7SST |
| WPX serial fit | F6FVY's data in MRCE `CqWpx.pas` | two derived numbers |
| Names | SSA baby names | public domain |
| DX names, reports, Sprint fit | written for Dust | GPL-3.0-or-later |
| Caller behaviour | MRCE (MPL-2.0) | re-implemented, credited |

MRCE's files attach no MPL Exhibit B, so porting its code into a GPL work
would be allowed (MPL-2.0 §3.3); Dust re-implements it by choice. Credits
(`src/ui/credits.rs`) name MorseRunner (VE3NEA, MorseRunner CE), the
compilers of the counted files, Morse Walker, AD1C, the SSA, the FCC and
F6FVY.

## 12. Code structure

**Interfaces.**

```rust
pub trait CallSource {
    fn next_call(&mut self, s: &TrainingSettings, spec: CallSpec,
                 avoid: &dyn Fn(&str) -> bool) -> GeneratedCall;
    fn learn(&mut self, answered: Answered<'_>);        // session.rs:97-102
}
#[derive(Clone, Debug, PartialEq)]
pub struct Turn { pub id: u64, pub at_run_ms: u64, pub segments: Vec<Segment> }
#[derive(Clone, Debug, PartialEq)]
pub struct Segment { pub who: Sender /* You | Caller(u32) */, pub text: String,
                     pub voice: StationVoice, pub start_sec: f64, pub end_sec: f64,
                     pub fade: Option<FadeKey> /* from phase 2 (5.5) */ }
pub struct Fields { pub call: String, pub exch: String,
                    pub call_edited: MonoMs, pub exch_complete: Option<MonoMs> }
pub struct ContestView {
    pub phase: ContestPhase, pub clock: Option<RunClock> /* None in the countdown */,
    pub limit_ms: u64, pub my_wpm: f64,
    pub qso: QsoState,         // last call sent, exchange taken, corrected, F3's call
    pub air: Option<AirView>,  // started, your text, segment spans
    pub log: Vec<LogRow>,      // oldest first: n, time, call, fields, wpm, asked, Pending | Checked
    pub qsos: u32, pub clean: u32, pub rate: Option<f64>,
}
```

Effects carry `Turn`, which derives `PartialEq` (as `StationVoice` does,
`station.rs:12`), so machine tests compare effects as the group tests do
(`machine.rs:622-628`). The runtime calls `plan_turn(&turn, &settings)`,
then `lease.send_planned(asked, &plan, &transcript, &settings, on_start)`
with `on_start: impl FnMut(f64)` taking lead plus latency in seconds, and
`asked` from `lease.ask_to_send()` as the turn goes on the air (5.2). The
view holds no caller truth until a row's check resolves, and is published
only when it changes (`session_runtime.rs:297-303`).

`ContestEntry`, in `AppCtx` beside `answer: AnswerOutbox`, holds `call`
and `exch` with their edit stamps (one `CopyValue<Fields>`, stamped by
`Fields::edit`) and `focus` as `CopyValue`s; `typed: Signal<u64>`, bumped
on every edit and read only by the hint, label and "Reads as"; and
`repaint: Signal<u64>`, read only by the inputs and bumped only by keypad
or paddle letters, Wipe and `clear()`. Every key and message button goes
through one entry point, `press_key`, which calls `paddle.finish_letter()`
(lent by the run screen through `ContestEntry::use_letters`, as
`AnswerOutbox::use_entry` lends it), `lease.wake()` and
`entry.snapshot()`, then dispatches `Key`. `run_effects` handles
`ClearFields` and `Focus` through `ctx.entry` and the focus script
(`widgets.rs:67`, shared with the group boxes); `Announce` sets
`ctx.announced`, which the screen reads. One `use_answer_box` registration
routes paddle letters to the focused field. `app.session` holds
`Live::{Groups, Contest}`, each boxed; `finish_run` and `abort_run` reach
whichever is live.

**New in cw-core, `crates/cw-core/src/contest/`:**

| File | Holds |
|---|---|
| `mod.rs` | `ContestKind`, `RunStyle`, `CallerHabits` (forgiving serde), `CallSource` |
| `rules.rs`, `esm.rs`, `clock.rs` | the `ContestRules` table; `esm()`, `Fields::edit`; `RunClock` |
| `machine.rs` | `ContestMachine`, events, effects, `ContestView` |
| `caller.rs` | caller states, patience, matching, replies, collisions, QRS |
| `turn.rs` | `Turn`, segments, continuations, `plan_turn`, per-caller fades |
| `place.rs` | `place_contest_callers` (phase 2) |
| `exchange.rs`, `parse.rs` | canonical forms, cut numbers, keyed tokens; `parse_exchange` |
| `serial.rs`, `check.rs`, `score.rs` | serials; `QsoCheck`; `contest_score`, rates |
| `record.rs` | `ContestLog`, `ContestRun`, `QsoRecord`, `result()` |
| `bot.rs` | operators that hear the turns; Hear a QSO uses the perfect one |
| `data/` | `names`, `places`, `zones`, `sections`, `fd`, `power` |

**Changed in cw-core:** `Cargo.toml` (`serde_json`); `settings/mod.rs`,
`contest.rs` (new), `save.rs`, `reset.rs`, `clamp.rs` (`Contest`, `ALL` of 6,
`draws_characters`, `sends_callsigns`, `ContestSettings`, `contestActive` on
disk, `Station`, `wild`); `settings/curriculum.rs` (Contest → `LevelAxis::Callsign`,
`:49-56`); `session.rs` (`StoredSession`, `mode_on_disk`, `recorded_mode`,
`contest`, a test constructor, `MonoMs::plus_sec`); `callsign.rs` (`generate_call`, `CallSpec`, public
`parse_callsign`, `wpx_prefix`, `country_of`); `sampling.rs`, `pool.rs`,
`auto_level.rs` (Contest arms; `impl CallSource for Sampler`); `stats.rs`,
`board.rs`, `speed.rs` (history rows, kinds, assisted runs, contest speed
evidence, no adaptive path); `timing/pileup.rs` (reach helpers
`pub(crate)`); `soak.rs`, `lib.rs` (the contest soak; exports).

**New in the app:** `src/contest_runtime.rs` (`LiveContest`, start,
dispatch, effects, `play_turn`, finish, the heartbeat check);
`src/ui/contest.rs` (run bar, hint, air line, fields, key bar, log, paused
panel, the visibility listener and the minimise handler); `src/ui/contest_keys.rs`; `src/ui/contest_results.rs`;
`src/ui/settings_station.rs`; `src/ui/scores/runs.rs` (phase 4);
`scripts/contest-data.py` (counted shares).

**Changed in the app:** `src/session_runtime.rs` (`Live::{Groups, Contest}`,
`start_session` routing at `:180`, `finish_run`, `abort_run`);
`src/state.rs` (`contest: Signal<Option<ContestView>>`, `ContestEntry`,
`session_running`, the restore message); `src/routes.rs` (Training and
Results branches); `src/app.rs`, `src/ui/tour.rs` (Exit and Discard,
contest keys, tour button); `src/keep_awake.rs`; `src/player.rs`,
`src/audio/mod.rs` (`send_planned`, `start_planned`, `Lease::wake`, a
recorder that keeps each start's plan and `Qsb`); `src/audio/mixer/*`,
`src/audio/web/*` (the dry path, later the gate); `src/ui/training.rs`,
`widgets.rs` (`Keypad` rows as props, `ProgressHeader { figure, pct }`);
`settings_charset.rs`, `settings.rs`, `home.rs`; `results.rs`,
`heatmap.rs`, `stats_detail.rs`, `scores/mod.rs` ("Points", rows that open
results, kinds); `listen.rs`, `playback.rs`; `privacy.rs`, `credits.rs`,
`docs/privacy.html`; `src/persist/backup.rs`; `assets/styles.css`;
`docs/platforms.md` (new modes write a legacy name plus a mark);
`scripts/callsign-prefixes.py` (zones and continents).

## 13. Testing

**Unit tests (cw-core).**

- **Parse:** every row of 11.7; fuzzing (no panic, idempotent on its own
  output); every equality in 11.6; no cut letter outside a numeric field.
- **ESM:** each row of 4.4 with its label, the states `C N V̄` and `C N̄ V̄`,
  Invalid sending nothing, focus moving to Exchange, `Fix call` after a
  correction.
- **Matching:** exact, close, none; `?` inside and at the end; best
  confidence only.
- **Callers:** patience over 10,000 draws in 4–14, mean near 6; timeouts
  within ±50%; nothing sent after `Done` or `Gone`; CQ fails the worked QSO;
  a worked station leaves after 3 silent timeouts; each message answered
  while calling costs a try; a `?` after someone else's call is not for a
  caller, unless the Call field is empty; a partial while worked brings the
  call; an exchange after an answered partial is for the caller; a piece of
  the call asks, and a bare `?` is nobody's call.
- **Calls:** a golden list keeps `generate_callsign` unchanged;
  `parse_callsign` reads 10,000 generated calls per tier; each `CallSpec`
  gives only its countries; every WPX prefix case in 11.1.
- **Data and scoring:** the NA zone and province tables, LB, DC and the
  Sprint list pinned; 85 sections, named by list version; hand-worked logs
  per contest; verified ≤ raw; a dupe scores 0, is no error, counts clean;
  KL7 and KH6 are CQ WW countries.
- **Turns:** `wanted` ends last; no reply is cut; your tones are unfaded;
  callers fade independently and smoothly; a continuation keeps every
  character of your message; capacity follows reach ÷ 60 Hz + 1.

**Machine tests** (fixed seeds, a script source):

- the basic QSO; `?` on an empty exchange; Again adding a send, a timeout
  adding none; a pile-up caller worked on its third CQ keeping 1 send; a
  corrected call keeping the first call as its copy, with no send added;
- F5 then F2 during your call sends both; a TU keyed during replies is
  finished and checked; Esc resets C and N;
- Esc, Pause and Wipe never give `Paused{Sound}`; only a current-id
  `TurnFailed` does, near the limit too, with retry time left out;
  `TurnStarted` emits nothing;
- pause in the countdown; resume re-arms the run end; a key that sends
  forgets the resume timer; `Hidden` never resumes by itself;
- Again ×3, Wipe, CQ brings a new caller; a wipe after hearing a call adds
  a lost group; a NIL adds one call group; an empty run stores nothing;
- a continuation keeps every character wherever it is keyed; an exchange
  sent with a logging TU carries that QSO's serial; a row's check shows once
  its TU is out; a repeat costs its send once it begins; Esc takes back what
  had not gone out; a key that sends nothing keeps the resume timer; a
  pause between typing and the key leaves the copy time alone;
- a CQ or TU ends the QSO for Enter; an exchange nobody took is owed
  still; after a correction Enter waits for the call; a partial while you
  work a station brings its call; CQ after CQ moves the frequency on;
- one station has one copy (a call logged again, lost then logged, logged
  with nobody heard, a dupe); a NIL is never charged to the next caller; a
  number logged before it was heard is a miss; a lost call keeps its copy
  time;
- asking during their first exchange costs its send; a call heard once
  costs one send; a repeat costs its send whatever key draws it; a
  question after the exchange is copied leaves its copy alone;
- a CQ stopped before it went out brings no caller; Esc takes back an
  accidental CQ; Wipe then Esc brings nothing back; a forgotten timer
  waking mid-turn leaves the retries out of run time; pause and Resume is
  no free `?`;
- the run end waits for a sound being retried; logging without sending
  counts what has reached you first; a pause just after Resume keeps who
  was cut short; a repeat cut before it began is charged once heard; a
  pause you choose mid-send brings no quick repeat; a message without a
  call goes with the call last sent to the station; a NIL matching nobody
  goes only to the newest station heard; a partial while the station waits
  is charged and timed;
- a TU without logging then a log is checked against that station, and Esc
  after it leaves that QSO to log; your exchange after an answered partial
  is answered; a question with the Call field empty brings the caller back;
  a piece of a call sent alone asks as a partial; a lost QSO keeps the call
  first sent with your exchange; Esc takes back nothing heard at an earlier
  key; a CQ that brings a call again costs its send; the station you work
  leaving is said, and Enter starts afresh; asking again for a call already
  typed costs it nothing; a bare `?` in the Call field is a question.

**Operators hear, never peek:** they read callers' text from `PlayTurn` in
cw-core and from the recorder's transcript in the app. Perfect: all
verified, at 3600·n / Σ(planned turns and drawn delays) an hour within 5%,
about 230 at 25 WPM for WPX. Human-paced (1.5 s per character typed, and
per key pressed): 60–150 an hour.
One digit off: verified 0. Unsure: asks `K5?` first, its call points
halve. Pauser: the clock equals the unpaused time. Dupe worker: 0 points,
no error.

**Soak** (`soak.rs`, its own 2,000 seeds). Keys carry fields 40% as heard,
20% one edit off, 10% empty, 10% partial and 20% junk, among stale and
current turn events, timers by any id, pause, resume, hide and finish.

- Every step: no panic, fresh ids, a clock that never goes back, but for
  the retries a failed send gives back as paused (4.2).
- The end: serial = QSOs logged + 1; QSO times rise and stay within
  `run_ms` ≤ the limit; `run_ms + paused_ms` is the wall-clock span, or at
  most that when time ran out, as a run noticed past its limit ends at the
  limit (6.5); no reply ends after `wanted`; no row is `…`; `g` sums to the
  groups; no station's groups are owned by two QSOs; points finite;
  verified ≤ raw; `result()` round-trips.
- Across seeds, a floor of clean, busted, field-error, NIL and lost QSOs.
- Each seed's (event, now) list replayed twice gives the same run.

**Storage.** A run writes `"charSetMode":"callsign"`, the mark and
`contest`. A frozen copy of today's `SessionResult` reads it as Callsign and
drops the log; the new build reads that as a run by its mark. A `v: 2`
object with renamed keys, and `{}`, stay unread and `to_value(written) ==
input`. Golden runs of each shipped phase score the same in later phases.
Settings round-trip Contest through `callsign` plus `contestActive`. Merge
fills and counts. 12 WPX QSOs stay under 6,000 characters, plus under 350
per further QSO.

**Runtime tests** (fake player): a full run stores a log and shows
Results; Exit with one QSO shows Results; the recorder sees QSB `None`,
`wanted` last and no reply cut; a cancel never pauses, a failure does;
`on_start` dispatches `TurnStarted`; pause releases the audio and sends
`keep_awake(false)`; `Paused{Sound}` then Resume opens a player before the
1 s timer; a key typed between two renders survives; `Pause{Hidden}` and a
heartbeat gap both credit the gap as paused; a bad own call refuses to
start; a turn waiting out a retry never starts over a newer one, and an
Esc before a turn's task runs stops it. A unit test drives
`audio::silent::MorsePlayer` through a stretched turn and a cancel.

**Render tests:** Home in Contest with and without a call; no alphabet,
group, Experience or Auto level controls; Your station's fields and
reading; the run screen in countdown, running, corrected and paused; hint
and Enter label per ESM row; the keypad without `=`, `+`, `,` and `.`; the
longest Simple QSO and Sprint lines at 360 px; log rows with text; results,
"Points", History rows; the 3 + 3 grid.

## 14. Phased build plan

**CI** runs on every merge: `cargo fmt`; clippy with `-D warnings` for
desktop (`--all-targets`), wasm32 web, and mobile-silent (`--all-targets`);
`cargo check` for `aarch64-linux-android` and `aarch64-apple-ios-sim`;
tests for cw-core, desktop and mobile-silent (`.github/workflows/ci.yml`).

**Staging.** `dust` is a binary crate, so a `pub` item reached only from
tests is dead code and fails clippy. So:

1. Every cw-core contest item is `pub use`d from `lib.rs` as it lands.
2. App code lands only with a caller in shipped code: the Contest arm of
   `start_session` and Contest branches in `TrainingRoute` and
   `ResultsRoute`. Tests set the mode directly, so the code is reachable
   while `CHAR_SET_MODES` lacks Contest and no user can pick it.
3. No `#[allow(dead_code)]` scaffolding.

Each phase and slice merges green and leaves the app releasable. Inside a
slice, commits go cw-core leaves, machine and soak, settings, player and
runtime, then screens.

**Phase 0: history that lasts.** `CharSetMode::Contest`, not selectable,
with arms that behave as Callsign (`settings_charset.rs:123-142` has an
exhaustive match); the `StoredSession` mirror, `mode_on_disk`, the mark and
the opaque `contest` value; `GroupSession::result` mapping Contest to
Callsign; settings that name Contest read as Callsigns until phase 1
writes `contestActive`; a mode label on every History row; and a
`SessionResult` test constructor the app can reach (cw-core's `empty_for_test`,
`session.rs:667-687`, is `#[cfg(test)]`), so the two app literals that
name every field (`render_tests.rs:155`, `persist/mod.rs:256`) stop doing so. The user sees each
session's mode, and no newer history loses anything on this build. It
ships a release before phase 1.

**Phase 1: single caller,** in four slices.

- **1a First QSOs.** WPX; Your station (call only); `contestMinutes`; the
  run screen with `ContestEntry`, hint and keypad; ESM; CQ, Again, Wipe,
  Esc; continuations; pause by release and claim; keep-awake on a live run;
  time, QSOs, the live log; results with both WPX scores, reopened from
  History; `ContestLog` storage with lost copies, keyed tokens and asked
  sends; "Points"; the merge; the Home hero; Exit and Discard routing; the
  privacy texts. Your message goes through the receiver, QSB off. The
  engine came with `TurnFailed` → `Paused{Sound}`, so the runtime brought
  `Lease::wake`, the hidden-page pause by visibility, minimise and
  heartbeat, and heard time with latency into 1a as well. Its cw-core
  foundations land first, with the settings, which then read
  `contestActive` back as Contest; until the Contest arm of
  `start_session` followed, `start_session` ran such settings as Callsigns
  at the same tier, and that arm removed the shim.
- **1b Honest clock and sound.** The dry path for your message; rate;
  − / +.
- **1c Simple QSO.** Names data, the RST and NAME exchange, `myName`.
- **1d Contester keys.** F2, F3, F5, `;`, `.`, `,`, `+`, Shift+Enter; the key
  bar; browser defaults suppressed; Esc lock in fullscreen.

**Phase 2: pile-ups and Sprint,** the first release.

- **2a Pile-ups.** `contestStyle`; arrivals from the stations range;
  `place_contest_callers`; per-caller QSB; best-match partials; collisions
  carried through continuations; NR?/AGN; calling twice; dupes, B4 and NIL;
  F4, F6, F8, F12; "2 calling"; the Band card labels.
- **2b Sprint.** `myState`; US and Canadian places on the fallback shares;
  the Sprint serial fit; the deterministic parser; Sprint scoring, with the
  multiplier list checked against the current NCJ rules before it ships.
- **2c Learner aids.** Hear a QSO with the perfect operator; presets; QRS.
- **Then** the Contest segment joins `CHAR_SET_MODES`.

The user can work single callers and pile-ups in WPX, Sprint and Simple QSO
on a phone or a desktop.

**Phase 3: the other three contests.** ARRL DX, CQ WW, Field Day; the
station fields and your side; region-filtered callers; zones, sections,
class and power shares; FCC counts in place of the fallback; both scripts
and a regenerated `callsign/prefixes.rs`. Every table line is checked
against the published rules and pinned before it ships.

**Phase 4: realism and aids.**

- `contestHabits`: numbers as MRCE's callers send them (`real`); LIDs and
  QRM (`rough`). LIDs send a wrong digit then `EEEEE`, send their exchange
  early, or accept a wrong call 4% of the time (`Station.pas:509-521`,
  `DxOper.pas:651-671, 879-886`).
- `contestShowCopy`, the screenshot's Preview Mode: each caller's text shows
  once its send ends. An item's stats copy is frozen when its text appears
  (received = the field text then; the reveal counts as a send); later
  edits count only for the log. The run is marked `contest:<kind>:a` and
  stored `assisted`: in every stat, greyed and unplaced on the board.
- The Runs card.

**Phase 5: sound and Listen.** The receiver gate; flutter; a timed splice
for continuations; contest tour steps; adaptive caller speed from the Contest
copy-speed series; the paddles' field space.

**Later, not planned:** Search & Pounce with the Sprint QSY rule, QSK, RIT,
keying with the paddles, a contest level, and more contests.

## 15. Decisions

### 15.1 Design decisions

| Decision | Chosen | Over | Why |
|---|---|---|---|
| Mode | `CharSetMode::Contest`, a 6th segment | a switch on Home; a `practice` key | "instead of alphabet"; the compiler finds every mode reader |
| On disk | `"callsign"` plus a mark, sessions and settings | `"contest"` | old builds keep runs and open in Callsigns |
| The log | a raw JSON value, parsed strictly on demand | an untagged enum with catch-alls | a lenient struct reads any object and cannot write a newer log back unchanged |
| Unknown session keys | not kept | a flatten catch-all | it kept retired keys, broke a test and risked duplicate keys; the log is kept explicitly |
| First release | end of phase 2: pile-ups, Sprint | WPX single caller first | "contest and pile-up training"; the screenshot shows Sprint |
| Phase 1 | four releasable slices | one step | smaller merges; nothing waits on F-keys |
| A key mid-turn | a continuation; only Esc cuts | stop and restart | MRCE queues; a TU must not vanish; collisions stay audible |
| Send outcomes | `TurnCancelled` apart from `TurnFailed` | one event and the id | a cancel and a failure share the id |
| Start stamp | `TurnStarted` on every attempt, with latency | no restart event | one hook serves retries and latency; a repeat must not be charged before you heard the end |
| Own sending | receiver at `VOLUME_MAX` (1a), dry path (1b) | `volume_min`; dry path in phase 5 | `volume_min` is buried at hard settings |
| Pause | release the audio; Resume claims in the tap | stop the turn only | silence, sleep and the gesture rule at once |
| Hidden page | visibility, minimise, heartbeat | visibility only | a minimised window is never hidden; native never reports suspended |
| Time up | ends at once; a started TU is final | a 15 s Closing | a real log does not count an open QSO |
| Speed offset | for the run only | saved | no third speed setting with a cross-field clamp |
| ESM on a bad exchange | sends nothing, "Fix exchange" | a `?` | the fault is your typing; MRCE refuses it too (`Main.pas:1210-1216`) |
| Again | `?`, as MRCE's F7 | `K5X?` | partials go through Enter |
| Sends | repeats you drew: calls asked for, exchanges any key drew | every re-send | pile-ups and slow typists would pay for waiting; time charges waiting |
| A correction | no added send; the first call is the copy | an added send | one error is not punished twice |
| Lost copies | a call group per station engaged | logged copies only | wiping a hard call must not raise accuracy |
| Fields logged before they were heard | a missed group each | no group | logging early with a guess must not raise accuracy, nor the board's points per group |
| A send a pause you chose cut | owed on the caller's timeout | sent again a moment after Resume | a pause is no free `?`; a hidden page or failed sound is no choice, and does send it again |
| F3, then log | checked against the station F3 closed | a NIL | MRCE's log finds a station done with you (`Log.pas:745-755`) |
| Stored tokens | as keyed | canonical | Letters and Mistakes see what was heard |
| Copy speed | calls and numbers only | every group | closed lists are guessed, not copied |
| Board | per group within a kind; full runs ≥ 5 min | one Contest pill; 10 groups | exchange length would order it; 3 QSOs could top it |
| Labels | "Points"; "QSO pts · Mults · Score" | "Score" for both | two scores, two words |
| Keys | Tab as on any page; `;`; Alt+S, Alt+E | MRCE's Tab cycle, Insert | no trapped focus; screen readers keep Insert |
| Difficulty | presets writing shared fields | a contest level | a level would own stations and tier |
| Your call in a run | not stored; side, country, continent, zone are | stored | no score needs it |
| Numbers | yours cut (Sprint plain); callers' by habits | always cut | what operators send; learners start plain |

### 15.2 Decisions taken with the user

The user approved every recommended default on 2026-10-20:

1. Contest is first offered at the end of phase 2, with pile-ups and Sprint.
2. "Repeat their call" is `?` on the air, halving the points of what is
   re-sent.
3. "DX" is ARRL DX, your side from your call; "Sprint" is the NA Sprint
   exchange without QSY.
4. Runs teach the sampler and count in Letters and Mistakes: a letter is a
   letter.
5. Contest runs rank within their kind, not by points per character on the
   All board.
6. The speed offset lasts one run.
7. Builds older than phase 0 may drop logs but keep runs; the mark and
   backups recover them.
8. Data licences: asking AD1C about cty.dat facts and W7SST about the
   counted files is for the maintainer to do. Until they answer, the tables
   use facts and rounded counted shares only, as designed (11.8).

## 16. Risks

1. **Phone height.** *Mitigation:* fixed one-line slots, the log below the
   fold, keys that shrink at 760 px, a 360 px render test, a small phone.
2. **A continuation re-sends one character** until phase 5's timed splice.
   *Mitigation:* the message counts as sent whole; listen on each player.
3. **Latency estimates** may be wrong. *Mitigation:* capped at 0.5 s
   (`wait.rs:114`); measure on low-end phones.
4. **Hidden-page detection.** *Mitigation:* three signals; test a locked
   Android phone, an iPhone and a minimised Windows window.
5. **Browser keys.** F1, F5 and F12 may be refused. *Mitigation:* a document
   listener; every key has a button.
6. **Pre-phase-0 builds** file runs as Callsign, move Callsign speed and drop
   logs. *Mitigation:* phase 0 a release ahead, the mark on upgrade, the
   merge, release notes.
7. **Web storage** holds fewer than 500 heavy contest sessions.
   *Mitigation:* the 60-minute cap, short keys, the squeeze notice.
8. **Wrong rules or tables** teach wrong exchanges. *Mitigation:* versioned,
   pinned tables, and simplifications stated on the card.
9. **Parser ambiguity** (`TN`, `AL`). *Mitigation:* closed lists first, the
   reading shown before logging, and Invalid sends nothing.
10. **Two scores** may confuse. *Mitigation:* they never share a word, and
    the results explain them in one sentence.
11. **One station means an empty band** (`activity.rs:43-50`).
    *Mitigation:* "Band activity" says so.
12. **Sampler drift** to characters a Koch learner has not unlocked.
    *Mitigation:* as Callsigns mode does today; the snapshot shows it.
13. **Machine size.** *Mitigation:* pure modules, slices, operators, and a
    soak that asserts coverage.
