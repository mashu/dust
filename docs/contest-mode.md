# Contest mode

Status: being built phase by phase (§14). Phase 0, history that lasts, has
landed: Contest exists as a mode, runs written by later
builds are read and kept, and History names every session's mode. Phase 1's
cw-core foundations are in: the WPX rules row, contest calls and their
parsing, exchanges and cut numbers, ESM, the run clock, callers' serials,
checks, both scores, the run's log layout, and the contest settings. So is
its engine: single callers, turns and `plan_turn`, the run machine and what
a run stores, with operator tests and a soak. The app runs it from start to
results (`src/contest_runtime.rs`) on the run screen of §7
(`src/ui/contest.rs`, its keys in `src/ui/contest_keys.rs`), with the
keypad, pause, the hidden-page pause and the sound failure pause, and shows
the contest results (`src/ui/contest_results.rs`), also from History. So do
the settings in Contest (the Character set card's contest, Your station in
`src/ui/settings_station.rs`, and the controls a contest hides), the Home
hero, the stats pills and notes, the privacy texts and the backup merge. Slices 1b, 1c and 1d have landed too: your own message
played dry, around the receiver at a fixed monitor level (5.3); Simple QSO,
with its names and reports (`contest/data/`) and `myName`; and the
contester's keys with their buttons on the key bar. Slice 2a has landed
too: `contestStyle` and pile-ups (`contest/place.rs`, the callers' fades in
`contest/turn.rs`), with collisions, best-match partials, missed exchanges,
calling twice, dupes, and F4, F6, F8 and F12. Contest is in the mode
picker from slice 2a on, ahead of the rest of phase 2, so it can be tried
while Sprint and the learner aids are built (§14). Fixes from an
end-to-end test of the web build in a phone-sized browser followed: one
press of Start or Listen after a refusal, each screen opening at its top,
Discard's second question safe from a double tap, the cursor kept by
Discard, Keep it and the theme button, and the run fitting a 360×640
screen (§2, §7). Fixes from an audit of pile-ups followed: going on to
another caller mid-QSO is a transition of its own (§4.4, §9.1), a caller
that only answered your partial is no QSO (§4.7), and callers' timeouts
overlap, planned into the turn on the air so that none cuts it (§4.3).
Slice 2b has landed: the NA Sprint, with `myState`, US and Canadian
places (`contest/data/places.rs`), the Sprint serial fit and the
deterministic parser (§3, §8, §11.2, §11.6, §11.7). Fixes from a second
audit of pile-ups followed: callers' patience follows MorseRunner's for a
partial, for being keyed over and for coming back from waiting out
another QSO, and a correction after your exchange carries the station's
own (§4.3); Enter and the screen follow what of the turn has reached you,
and a check shows once your TU is out (§4.2, §4.4, §4.8); the band makes
room only for the callers sounding at once (§5.6); and a QSO logged for a
station you left is no NIL of the one you work (§9.1). Slice 2c has
landed: Hear a QSO, three QSOs of the chosen contest worked by the perfect
operator, on Listen and on the Character set card (§2, `hear_qsos` in
`contest/bot.rs`); the First QSO, Club and Contester presets (§8,
`settings/contest_preset.rs`); and QRS, a caller slowing down after a few
incomplete tries (§4.3). With them, two fixes from browser tests: what
Start keeps from Home's hero is stored as the run starts (§2), and the TU
that logs after a TU without logging asks no caller to call again (§4.6,
§9.1). Slice 3a has landed, the first part of phase 3: CQ WW and ARRL DX
(§3), with each country's name, CQ zone and continent from cty.dat in the
call table and North America's zones by hand (`contest/data/zones.rs`,
§11.3), the ARRL DX side and Labrador (`contest/data/places.rs`, §11.2),
the counted power shares (`contest/data/power.rs`, §11.5), points by
distance and the multipliers of both sides (`contest/score.rs`), and
`myZone`, `myPower` and LB in `myState` on Your station (§2, §8). Fixes
from a third audit of pile-ups followed: B4 sends a dupe its call again
when it never heard it (§4.7); after Esc the callers answer what they
heard as garbage, at once, and what reached you of a caller you keyed
over stays heard (§5.2); the station you work, still waiting for your
exchange, falls silent and stays rather than leave (§4.3); Enter calls CQ
in place of any call the fields still hold for a station that has gone,
or clears it once someone calls, and the hint says so (§4.4, §7); and the results read each QSO at the speed its live row
showed, while a correction's exchange is a copy only once it begins to
reach you (§9.1). Slice 3b has landed, the rest of phase 3: Field Day (§3),
with the 85 ARRL and RAC sections (`contest/data/sections.rs`, §11.4),
the class, section and power shares counted by `scripts/contest-data.py`
into `contest/data/counted.rs` (§11.5), the section a call tells by itself
(`call_section`, `contest/data/places.rs`), a caller's class and section
(`contest/data/fd.rs`), a score of QSO points with nothing to multiply
them, and `myClass` and `mySection` on Your station, which Home asks for
as it asks for a state (§2, §8). Every contest of the card has shipped;
two checks stay open, Yukon's section code and the FCC licence counts
(§14). Slice 4a has landed, the first part of phase 4: `contestHabits`,
easy, real or rough, on the Band card in Contest mode and written by the
presets (§8); real habits sending numbers as MorseRunner's callers do,
checked against its `Station.pas` (§11.6); and rough habits adding
careless callers and on-frequency QRM (`contest/qrm.rs`, §4.3, §5.6).
Slice 4b has landed, the rest of phase 4: `contestShowCopy` on the Session
shape card in Contest mode (§8); with it on, each station's text in the RX
line a moment after its send ends (§7), each copy kept as it stood when its
text showed, the showing a send of its own, and the run marked
`contest:<kind>:a` and stored `assisted` (§9.1), counted in every stat and
listed greyed, never placed, on the board (§10); and the Runs card, every
run of a contest on Stats → Scores under the Contest pill (§10). With them,
two fixes from a browser test on a 360×640 phone: Pause keeps its place in
the row while paused, inert, so a double tap on it never ends the run, and
Resume brings the run screen back to its top (§2, §7). A fix from a third
browser test followed: Enter on the call of a caller that gave up before
you worked it, typed late, no longer sends it to nobody while the rest of
the pile-up waits on that QSO in silence, but clears the fields, or calls
CQ while nobody calls, as it does for the station you work once it has
gone, and so does Enter on a partial or a slip of it once that has gone
out to nobody and nobody calls; a caller that gives up on your partial
is said to have gone, as one is on its timeouts (§4.4, §7). A review of
group training against the main branch followed, as Contest must leave
group training as it was: Callsigns mode and the band's traffic draw
KG4 with two letters again, which only a contest's callers leave out
(§11.1, §11.2), and between two groups the training screen's header says
"Next group…" beside a plain dot while the box waits, not "Your turn".
A browser test of group training on a 360×640 phone followed, which found
the keypad's last row ending 16 px under the screen's foot, on the main
branch too: the run's rule for phones under 760 px tall is a session's as
well now, its groups drawn closer with its cards, and a long group
answered keeps to its slot (§7). Feedback from a contester running
pile-ups at 235 QSOs an hour on the published app followed, decided with
the user on 2026-10-22 (§15.2): Space in the Exchange field goes back to
Call, as MorseRunner's and N1MM's space bar does, but where it separates
the items of a longer exchange (§4.5); your sending has a volume of its
own, from off to the loudest, on the Tone and volume card (§5.3, §8); your
speed is a setting of its own, kept from run to run as − and + leave it,
the callers' speeds staying the Speed card's (§6.6, §8); and how often
your TU signs with your call in a pile-up is yours to choose (§3, §8).
Phase 5's adaptive caller speed followed, brought forward at the user's
asking and decided with the user on 2026-10-22 (§15.2): each new caller
comes at a speed drawn from your copy speed in contests, within the Speed
card's ranges, faster as you copy calls and numbers clean and slower as
you miss them, by the Bayesian belief group training's adaptive speed
uses, started from the Contest copy-speed series and learning within the
run; a switch of its own on the Speed card, on as it ships (§6.6, §8,
§10). Slice 5a has landed, phase 5's sound: without full break-in, as
MorseRunner ships it, the receiver is muted while you send, after its
AGC, on both players, and nothing a caller sends across your message
reaches you, for Enter, the copies or Show copy (§5.3); "Full break-in
(QSK)" on the Tone and volume card in Contest mode hears the band through
your message, as runs did before (§8); flutter, three callers in ten
fluttering at 3 to 33 Hz under the Band card's flutter in Contest mode,
which the Contester preset turns on (§5.5, §8); and a continuation spliced
on where your message reaches the start of its next character that the
player can still take over at, each player starting it at that moment, so
no character goes out twice (§5.2; risk 2 of §16 narrowed to a lead
underestimated). Slice 5b has landed, the rest of phase 5's learning
items: the guided setup in Contest mode is a run's, its button shown there,
walking the run screen part by part where it is drawn and leaving the run
through Discard, its Next never starting a run, whose callers would be
sound nobody asked for (§2); and the paddles' field space, a word space of
quiet after a letter pressing Space by the Space key's own rule, so a call
and its exchange keyed on the paddles land in their own fields (§4.5).

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
`SessionResult`, and so is each station you worked but lost, its fields as
misses, so every stat counts contest practice with no special code. A
compact log rides beside the groups. Older builds keep the run as callsign
practice.

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
`(CharSetMode::Contest, "Contest")` (`src/ui/settings_charset.rs:51-60`).
`.segmented` wraps; the picker's `.charset-modes` sits the six segments
three and three at 600 px and below, so Contest is never alone on a row.

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
  on the Band card"; as built, Single caller says "One station calls at a
  time") and presets (§8);
- as built (2c), "Hear a QSO" beside the QSO's eyebrow, and the presets
  after the QSO, above the tier, so what a preset sets shows right under
  it.

The alphabet controls are not drawn: the `if` blocks at
`settings_charset.rs:171-275` gain a Contest arm.

**Your station.** A new card under it, in Contest mode only, with its own
reset: your callsign and only the fields the chosen contest sends. Under the
call it shows what was read ("Germany · Europe · CQ zone 14 · you are DX")
or a plain error ("needs a digit", "unknown prefix"), and "No licence yet?
Make up a call from your country's prefix, a digit and two or three
letters. Stored on this device only." Phase 1 had only the country key and
the WPX prefix to read ("Reads as DL · WPX prefix DL1"). As built (3a), the
line names the country the call is at home in, as cty.dat names it
(`home_country`, which places the prefixes of Alaska and Canada the call
table lacks, §11.2), its continent (by prefix: European Turkey's TA1 is in
Europe) and the CQ zone the call reads as, "in California" where your
state and not the call alone gave it, then what the contest counts the call
by: its WPX prefix in WPX, your side in ARRL DX ("Reads as Fed. Rep. of
Germany · Europe · CQ zone 14 · you are DX"). Each part is kept on one
line, so a phone's line breaks only after a dot, as the results' error line
does. Your zone, in CQ WW, and your power, in ARRL DX from the DX side, are
fields of their own, each with a line under it saying how it goes out ("0
takes it from your call: zone 14, sent A4", or "from your call and state:
zone 3 in California"; "Sends 1TT: 100 W"). The errors are `call_fault`'s,
the reasons `parse_callsign` refuses a call ("Not a callsign yet: needs a
digit", "needs a prefix before the digit", "needs letters after the last
digit", "one slash at most"). As built (3b), Field Day's class and section
are fields of their own too, each with its line: "Sends 1D: 1 transmitter,
a home station on commercial power", or why what was typed is none ("1G is
not a class: type the number of transmitters, 1 to 99, and a letter A to
F, as 1D or 3A; 1D is sent"); "Reads as Eastern Massachusetts",
"Sends PAC from your call: Pacific" while you give none and your call
tells it, "Sends TER: ARRL contests count Yukon in the Territories" for
YT, or why what was typed is none ("MASS is not a section: type its code,
as EMA or ONS, or DX"). A call that reads but that Dust cannot place
says "unknown prefix, so no country" (and its WPX prefix in WPX), and
still starts a run unless the contest needs a zone or a state the call
cannot tell: CQ WW then asks for your zone, and the Sprint, or ARRL DX from
the W/VE side, for your state. With no call yet, the line says what a run
is missing, in `missing()`'s words ("Add your callsign to start a
contest").

**Home** (`src/ui/home.rs:128-216`) keeps its frame; its subtitle reads
"Call CQ and work the stations that answer — or just listen." The hero's
eyebrow reads "Contest", the level chip "CQ WPX", and field chips (`CALL`
`NR`) replace the pool chips, with "You send 5NN + serial, as DL1ABC".
While `myCall` is empty, or not a call a contest can read, the hero holds
an inline callsign field instead, writing the setting Your station writes
and kept the same way (`ContestSettings::set_my_call`), with the line Your
station gives a call typed under it. Specs show the callers' WPM range,
minutes per run ("10 Minutes", or "Open, up to 60 min") and "Single caller"
or "1–3 callers". Start reads "Start contest" (id `btn-start-training`); a
start that `missing()` refuses says why in the hero, above Start, unless
the line under the call field already says why the call will not do, and
when the call, the name, the state, the zone or the section is what is
missing, Start moves the focus to its field once the hero has drawn it (an
effect, `src/ui/home.rs`). Home asks for them in that order, one field at
a time (`StationAsk`): in a Simple QSO with neither call nor name, the
press that keeps the call swaps the call's field for the name's, and in a
Sprint the press that keeps the name swaps it for the state's. As built
(3a), CQ WW asks for your zone, "Your CQ zone", only for a call that does
not say it, and keeps 1 to 40 (anything else is 0, your call's), with its
own first step on the first contest's card ("Add your CQ zone, press Start
contest; …"). As built (3b), Field Day asks for your section, "Your
section", only for a call that does not tell it (a call in the US's
districts or Canada), after the zone's place in the order, with its own
first step ("Add your section, press Start contest; …") and, under the
field until a section is given, a line saying what one is ("Your ARRL or
RAC section, as EMA for Eastern Massachusetts; DX outside them"). While
the hero holds a field, Start and Listen keep the cursor in it and keep
what was typed there before they act, so a call typed after a refusal
starts the run on one press: leaving the box on the press would
keep the call then and redraw the hero, the button moved, under the finger
before the click came (`StationDraft`, `src/ui/settings_station.rs`). As
built (2c), the settings are stored as a session starts
(`AppCtx::store_settings`, from `start_session`), so a call, name or state
the press of Start kept is stored before the run, which stores no settings
while it is on: a reload mid-run used to lose it. The
runtime keeps its message for the other ways in, such as Run again. Contest
has its own first-session card ("Add your call, press Start contest; Enter
sends what the QSO needs next"), and its own tips (`CONTEST_TIPS`: Enter, a
partial call with `?`, and `?` for a missed number) in place of the
groups'. The level tile shows the callsign tier, and `AutoLevelCard` is
hidden, as for Words (`auto_level_progress` is `None` in Contest,
`crates/cw-core/src/auto_level.rs:220-229`).

**Listen** treats Contest like Callsigns (`src/ui/listen.rs:30-222`), as
does the one fork in group generation (`sampling.rs:503-507`). Phase 2 adds
"Hear a QSO". As built with adaptive callers' speed (2026-10-22), Listen's
stream of groups comes in Contest mode at speeds drawn evenly from the
Speed card's ranges, as it did before: the callers' switch is theirs alone
(§6.6), and group training's is not shown in Contest, so the sampler's
`GroupSource::next_speed` draws none there.

**Hear a QSO** (as built, 2c) is offered where a learner meets the
contest: first on Listen in Contest mode ("Hear a few QSOs of CQ WPX, or
play one character, the pool, or a stream of groups"), and beside the
QSO's eyebrow on the Character set card. It plays three QSOs of the chosen
contest, in its run style, speeds (drawn as a run's, §6.6) and band,
worked by the perfect operator
(`hear_qsos`, `contest/bot.rs`), which hears the turns only, as in a run.
They are worked through before the first plays, on a run clock of their
own, from a sampler built from the history as a run's is and let go of
after: nothing is stored, no stat hears of them, and History has no row
("Played for you to hear: not scored, not saved to History").
The turns go out through a run's player path (`plan_turn`, `send_planned`,
your message dry and the callers through the receiver, the send's own
fading off as a run's), each after the quiet the operator left before it,
and the dialogue shows each send as a line, You or They as the card's QSO
reads, once it has been heard whole (the attempt's lead and latency, then
the segment's end); callers sending at once for the same QSO share a line,
side by side, as a pile-up's do, and each QSO after the first is set apart
by a rule where its caller comes in (a QSO closes with your TU and its
sign-off). The eyebrow says which QSO plays ("QSO 2 of 3 · CQ
WPX"), with Stop beside it, and the lines scroll in a box of their own
kept on the newest, so neither Stop nor the line just heard is pushed
under a phone's navigation as lines come. They end on the third QSO's TU,
with its sign-off where the contest has one; every caller that TU brings
is left out, once or twice as it calls, as it would start a QSO not heard.
Stop ends it; a run's pause has no place in it, so a page out of sight
stops it too (a listener of its own on Listen and on Settings, beside the
run's, `src/page_hidden.rs`), and so, on Settings, does picking another
contest or another mode, whose card would no longer show it or its Stop.
While it plays it keeps the screen awake, as a live run does, so a phone
left to it does not lock and stop it. The lines stay once it ends, "QSOs
heard", until another sound plays, a character is picked on Listen, the
contest changes or the screen is left; stopped before a line was heard,
it leaves the stage or the card's QSO as it was. While the lines are up,
Listen's character strip picks none.
Where your station is not given, it stands in, and a line says so ("Your
station is sent as K1ABC MIKE MA until you add yours"): `DL1ABC`, or
`K1ABC` for a contest that sends your state or your section, `MIKE`, `MA`
and, for the section, `EMA` ("K1ABC 1D EMA")
(`ContestSettings::stand_in`, beside `missing`, which it answers).

**Navigation.** A run uses `Screen::Training` and `Screen::Results`
(`src/state.rs:19-26`), with the hidden nav, the refused `go_to`
(`state.rs:368-371`) and no settings saves mid-run. Four things change:

- Exit and the tour's Discard send a `SessionEvent` (`app.rs:184`,
  `tour.rs:222`) that only a group `LiveSession` receives
  (`session_runtime.rs:232-241`). Both go through new `finish_run(ctx)` and
  `abort_run(ctx)`, which reach whichever machine is live.
- Keep-awake follows "a run is live" (countdown or running) instead of the
  screen (`keep_awake.rs:17-27`), so a paused run lets the screen sleep; as
  built (2c), Hear a QSO playing holds it too.
- The tour targets group inputs (`tour.rs:231-240`), so its button was
  hidden in Contest mode until phase 5. As built (5b), Contest has a guide
  of its own (`CONTEST_STEPS`, `src/ui/tour.rs`), opened by the same
  button, shown in Contest now: Start contest, then each part of the run
  screen in turn, pointed at where it is drawn, by ids the run screen gives
  its parts (`run-bar`, `run-hint`, `run-air`, `run-fields`, `run-reads`,
  `run-keys`, `keypad`, `run-log`, `run-actions`): the status and clock,
  the hint, TX and RX, Call and Exchange, Reads as, the message keys or,
  where touch is how you type, the keypad (each build saying its own, as it
  does of what moves between the fields, Space or `␣ Field`), the log, and
  Pause, End run and Discard; then the run left by its Discard, asked
  twice, through `abort_run`, and a last word on where the rest is.
  Next never starts a run, whose callers would be sound nobody asked for,
  where the first session's Next starts the session: it moves on, and away
  from the run screen the steps point the way to it, Start contest on
  Practice and Practice from anywhere else, each saying above its words
  that a run is what shows it ("To see this, start a run: press Start
  contest."); Show me presses Start contest when asked to. On a paused
  run, whose card stands in for the hint, the air, the fields, Reads as and
  the keys, those steps point at Resume instead and say to resume the run;
  the clock, the log and the run's buttons are drawn paused and are
  pointed at as ever. On the run screen the guide docks at the foot while
  it shows the head of the screen, Resume among it, and over the head
  otherwise, and its buttons, the last step's Start practising among them,
  leave the cursor in the run's field, handing it back after a press, as
  the theme button does, so Enter goes on sending. What it points at is
  brought into the middle of what the guide leaves clear, not of the
  window, as a guide docked at the foot covers the middle of a short
  phone, and with room for the ring it draws; so is the Discard that asks
  once more, which stands lower than the Discard pressed, guide or no
  guide (`bring_into_sight`). Picking Contest while the guide is open, or
  another mode from Contest, starts that mode's guide from its first step
  (`Tour::contest`).
- Every screen opens at its top (`src/app.rs`), so a run started from the
  foot of its results begins with its clock and fields in sight. As built
  (4b), so does the run screen on Resume, by either way in (the button, or
  Alt+P), after a pause you chose, a hidden page or a failed sound: the
  paused page is shorter, so a phone scrolled down to Pause was left part
  way down it, and the run carried on with its header, clock and run bar
  above the screen's top. Pausing scrolls nothing, so nothing moves under
  the finger that paused (`scroll_to_top`, `src/app.rs`, from
  `resume_run`).

## 3. Contest catalogue

A contest is one row of a `ContestRules` table in cw-core: the CQ and TU
texts, your exchange, whether RST and a serial are part of it, the copied
fields per side, which callers answer, and the scoring rule. A new contest
is a row plus its data. In all of them you run (call CQ); callers send RST
as `5NN` (`Station.pas:529`), prefilled 599 and never checked, as loggers
do, except in Simple QSO; TU becomes `TU <your call>` in a contest whose
TU works as a CQ (not Simple QSO, whose TU greets the station by name and
brings nobody): as decided with the user on 2026-10-29, every QSO's, with
one caller at a time or a pile-up, unless you choose every third QSO, as
in MRCE's Pile-Up and WPX modes (`Contest.pas:468-474`), or never
(`contestTuCall`, §8, §15.2 item 14), as MRCE's station ID rate is a
setting (`StationIdRate`, `Ini.pas:259`); the
callers call after your TU whether it signs or not, as MRCE's do
(`Contest.pas:1000-1005` brings them on any TU), so a pile-up that never
hears your call goes on as ever; and
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
  `K5XYZ 5NN TT7` · `TU 5NN 214` · `TU DL1ABC`. They may also send
  `R 5NN 214`.
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
- **As built.** Your report goes out as `599`, as the template writes it,
  not cut. The caller's report is drawn from §11.5's shares and copied, and
  goes out as its habits send it (§11.6): plain digits under easy habits,
  599 as `5NN` or now and then `ENN` and any other with its nines cut
  (`57N`) under real and rough ones (as built, 4a). Asked again,
  the caller sends its exchange without `R TNX`; a correction is `DE K5XYZ`
  or `K5XYZ K5XYZ`, as in WPX. Your TU greets it by the name the Exchange
  field reads (`TU 73 EE` with none). Its `73 EE` comes in the TU's turn and
  brings nobody, and the hint then says "Enter calls CQ for the next
  station". A caller that has its call and not your exchange asks `AGN`,
  never `NR?`: there is no serial to ask for. The row holds the callers'
  texts as well (`answer`, `their_exchange`, `their_lead`, `their_query`,
  `signoff`, `tu_calls`), so a later contest is a row too.

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
- **As built (2b).** The current NCJ rules could not be reached from the
  build's network (ncjweb.com was refused, and could not be fetched
  either), so the multiplier list is this section's, which excerpts of
  NCJ's 2026 rules found by a web search agree with, as does a search
  summary of ARRL's notice of the contest; it is to be checked against
  NCJ's rules before a release, and the rules row says so. Those rules have
  a station outside North America send `DX` and one elsewhere in North
  America its country; Dust has every station outside the US and Canada
  send `DX`, a simplification the card and Your station say as one
  (`ContestRules::simplified`, `state_reading`). Callers are US stations, Alaska and Hawaii among
  them, and Canadian ones (`Countries::UsAndCanada`), not the islands
  `NorthAmerica` adds for Field Day: they have no state to send. Canadian,
  Alaskan and Hawaiian calls have two-letter prefixes, so they call from
  callsign tier 3 up. Your TU signs with your call every time, so a
  pile-up's every third TU adds nothing to it; their exchange has no word
  before it (`their_lead` empty), the caller's call having gone out when
  it called; the choice of your call in TU is not the Sprint's
  (`ContestRules::tu_call_chosen`). A station with no place sends `DX`.
  The card's pill comes
  first, as the card lists the contests (`ContestKind::shipped` keeps
  that order), and a line under "You send" states the simplifications.
  The dialogue's caller is `W7XYZ`, as the state it sends is Oregon.
  A QSO logged with `DX` counts a point and no multiplier. As built (4a),
  callers send their serials plain under every habit, as yours goes, so
  Real cuts nothing you copy here (§11.6).

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
- **As built (3a).** Your zone goes out cut and unpadded (`A4` for 14, `5`
  for 5), as MRCE sends yours; callers send theirs as their habits have
  them (`send_zone`, §11.6): plain digits under easy habits, and under
  real and rough ones cut by most callers and now and then padded (as
  built, 4a). F12 sends `NR?`, as MRCE's does in every contest. A QSO's
  points and country go by the call logged, placed where it is at home
  (`home_country`, §11.2), its continent by its prefix (European Turkey is
  in Europe): a call Dust cannot place is worth no points and no country,
  and from a call of yours it cannot place every placed station is on
  another continent. Your own country and continent are read the same way,
  so NL7AB is Alaska's and VG3ABC Canada's. Your own country is a
  multiplier too, as the contest has it. A call that does not say its zone
  (one Dust cannot place, or a Canadian call whose digit names no province,
  such as CH1 or VE0, until you give your province) needs `myZone` before a
  run, which Home asks for in a field of its own, and Hear a QSO stands in
  zone 14 for it. The card says what it leaves out: "Simplified: one band,
  so a station, a zone and a country each count once; and no WAE
  countries, only DXCC ones."

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
- **As built (3a).** Your side is read where your call puts you
  (`w_ve`, `contest/data/places.rs`): W/VE by every prefix the 48 states,
  DC and Canada issue, the table's or not, and DX anywhere else, a call
  Dust cannot place among it. The card's line under "You send" says it,
  from the row's `works`: "You are DX: you work US and Canada only", or
  "You are W/VE: you work DX only (Alaska and Hawaii count as DX)"; while
  your call does not read, "Until you add your call you are DX: you work
  US and Canada only". As DX your power goes out cut (`1TT` for 100 W,
  `KW` as written) and Your station asks for it, 100 W until you give it;
  as W/VE your state, and Home and Your station ask for it as in the
  Sprint. A W/VE caller from VO2 sends `LB`; a DX caller sends its power
  as written under easy habits, and cut under real ones (`send_power`,
  §11.6). Callers lead their first exchange with `TU` or `R`, as in WPX,
  and F12 sends `NR?`. A QSO is worth 3 points only with the other side
  (`Points::OtherSide`), by the call logged: one logged on your own side,
  a busted call among them, claims nothing even raw, neither points nor a
  multiplier. As W/VE, a call logged in W/VE counts no country; as DX, a
  state counts only from a W/VE call. With no call yet, Hear a QSO stands
  in as DL1ABC, on the DX side. The card says "Simplified: one band, so a
  station and a multiplier each count once."

### Field Day (`fieldDay`)

- **On the air.** `CQ FD K1ABC` (`ArrlFd.pas:434`) · `W7XYZ` ·
  `W7XYZ 1D EMA` · `R 3A OR` · `TU`. No RST. A class is the number of
  transmitters plus a letter A–F.
- **Rules.** 2 points per CW QSO (`ArrlFd.pas:493-497`). No power multiplier
  and no bonus points: those score the entry, not the copying.
- **Your fields.** Class (default `1D`) and section, defaulted from your call
  (§11.4).
- **As built (3b).** Callers are North American, KL7, KH6, KP4 and KP2
  among them at the call table's shares, and 0.9% DX, the share of
  Field Day's entries that send DX (`Countries::FieldDay`, which draws a
  call outside North America, `Countries::OutsideNorthAmerica`, that
  often). After review, the choice is made once per caller, before the six
  candidates (§11.1): made for each, the most wanted of them came out DX
  about 3% of the time, more for a learner weak at its rarer characters.
  A caller's class is drawn from the counted classes, and its
  section from where its call puts it (§11.4). They lead their first
  exchange with `R` nine times in ten, as MRCE's callers do, and `TU`
  otherwise; F12 sends `NR?`, as MRCE's does in every contest. The score
  is the QSO points, two a QSO: nothing multiplies them
  (`Multiplier::Nothing`, which counts the log as one multiplier, as MRCE's
  `ExtractMultiplier` returns `'1'`), and the results' Mults read `—`. The
  card says "Simplified: one band, so a station counts once; and no power
  multiplier or bonus points, which score the entry, not the copying."
  With no call yet, Hear a QSO stands in as K1ABC in EMA ("K1ABC 1D EMA").

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
  (1–5). Callers not worked keep calling while their patience lasts. As
  built (2a), the draw is capped at what the filter fits (5.4), and the
  callers already calling, the station you work among them, count. Your
  call alone (F4) brings callers too once a QSO is in the log, as MRCE's
  (`Contest.pas:1001-1025`). A pile-up of one caller is none, so picking
  Pile-up while the stations range holds one station opens it to 1–3 (8).
  As built (2026-10-23), the callers a message brings, and every other
  caller answering it, start no sooner than a station leaving has
  finished: one your CQ, TU or NIL sent on its way while it was still
  sending, which goes on with its rest (5.2), and its sign-off. They used
  to call across it, so a pile-up held to one caller at a time was heard
  as two, in about one run in seventeen worked at a person's pace.
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
- the log, your next serial, your speed (as built after operator
  feedback, §6.6; it was the run's speed offset), one id counter.

```rust
pub enum ContestEvent {
    Key { key: OpKey, fields: Box<Fields> }, // after a paddle letter finishes
    // each attempt, retries too: when its sound goes to the output, and
    // when it reaches the ear
    TurnStarted { id: u64, heard_from: MonoMs, output_from: MonoMs },
    TurnEnded { id: u64 },
    TurnHeldUp { id: u64 },     // its audio was held up: nothing in it is timed
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
| Countdown | Pause | timers forgotten, `ReleaseAudio`, "Paused" announced → Paused; Resume restarts the countdown, "Resumed" announced |
| Running, Silent | Key | ESM (4.4) or the key picks the message; a logging key logs (4.8); replies are decided, and the calls of callers whose wait runs out before the turn is over (4.3); a caller that answered your partial leaving on its last try, spent on your message, or a station gone whose call Enter clears, unsaid till then, is announced (4.4) → `PlayTurn` (new id) |
| Running, Turn | Key but StopTx | what of the turn has reached you counts first, so Enter decides on what you have heard; a continuation (5.2) → `PlayTurn` (new id); a key that sends nothing (Wipe, Shift+Enter, Enter on a bad exchange) leaves the turn playing |
| Running, Turn | TimerFired (heard) | what of the turn has reached you counts, a look at the clock (6.3): Enter's QSO and the screen follow the audio, a correction is said, and the checks whose TU is out show; the timer runs on to the next end of a send of the station you work, or to those checks → Turn |
| Running, Turn | StopTx (Esc) | `StopAudio`; C and N reset for a call or exchange of the message keyed last not fully sent (`Main.pas:884-891`), not for one carried from an earlier key, heard then; the callers hear only what of the message keyed last went out whole, and a QSO it lost or a caller it brought goes with it; your message cut short, the callers listening answer what they heard of it as garbage (5.2) → a turn of their replies, or Silent |
| Turn(id) | TurnStarted{id} | `heard_from` and `output_from` re-stamped; a second start, a retry, marks the turn untimed (docs/recognition.md); no effects, no limit check |
| Turn(id) | TurnEnded{id} | all segments count; each caller waits as the turn planned it (4.3); the patience timer starts if anyone waits → Silent |
| Turn(id) | TurnFailed{id} | time since the turn first started is credited as paused, but not time the clock was read at since (6.3), so the run bar steps back by the retries; every caller in the turn sends again after Resume, as it was keyed, so a repeat you asked for is charged once heard, but for a call on its timeout that never began, which waits afresh; timers forgotten; `ReleaseAudio` → Paused{Sound} |
| any | TurnHeldUp{id} | the turn is marked untimed, on the air or not; no effects |
| any | TurnCancelled, or another id | ignored |
| Silent | TimerFired (patience) | waiting callers call again (4.3); if the station you work has left, "K5XYZ has gone" is announced, by the call it put yours right with, or else the call you had for it, or "The station", and Enter starts afresh, as after a CQ, calling CQ in place of its call (4.4) → a turn of replies |
| Running | Pause{why, at} | `StopAudio`, `ReleaseAudio`; segments heard by `at` count. A send it cut before it began goes again after Resume as it was keyed, so a repeat you asked for is charged once heard, but for a call on its timeout, which waits afresh; one it cut after it began goes again, as no send you asked for, after a hidden page, but after a pause you chose it is owed on the caller's timeout, as one heard whole. A pause with nothing on the air keeps who an earlier pause or failure cut short. Timers forgotten; clock paused from `at` → Paused |
| Paused | Resume | clock resumes; run end re-armed with the time left; a 1 s resume timer → Running |
| Running | the resume timer | callers the pause or failure cut short send again, as the rows above say; the others wait for you afresh, as after a turn, so what was heard whole comes again only on a timeout or at your asking: a pause is no free `?`. A key that sends forgets the timer; one that sends nothing (Wipe, Shift+Enter, Esc with nothing on the air) leaves it to bring the callers back |
| Running | run-end timer, or the limit passed on any event but TurnStarted (a look at the clock, which stamps nothing, 6.3) | → Finished. While the turn on the air may be retrying its sound (an attempt after the first has started, or the one playing is past its length and 2 s), only what keeps its reading ends the run (its end, a key, End run): its failure gives the retries back (5.2), and the run end waits until its last attempt would have failed |
| live | Speed{±2} | your speed moves, from the whole speed nearest it, for your next message; the callers' stay the Speed card's (as built after operator feedback, §6.6) |
| live | FinishNow | → Finished |
| any | Abort | `StopAudio`, `AbortToHome` → Aborted |

**Finished** emits `StopAudio`. A logged QSO is final and checked, its TU
cut or not; any other open QSO becomes a lost copy (9.1). With no groups
(no station worked, though a NIL may have been logged with nobody heard),
nothing is stored and `ShowNothingKept` shows "No stations worked — nothing
kept" with Run again; otherwise `PersistAndShowResults`.

### 4.3 Callers

The callers' timings and states are modelled on how stations call in a
contest; MRCE's sources are the reference for the numbers below.

- **Reply delay** 0.1–0.6 s after your message (`DxOper.pas:225-232`).
- **Reply timeout** `(6 − skill)` s, Gaussian, within ±50%
  (`DxOper.pas:282-287`). Each timeout costs a unit of patience and the
  caller sends again. As built (2a), each caller's wait is its own, as in
  MRCE (`DxStn.pas:204-208, 292-301`): it is dropped while you send, and
  drawn afresh from the end of the caller's own send, or once it has heard
  you; another caller's send leaves it alone, so on a quiet frequency every
  caller calls in turn, not only the quickest. A wait that runs out while
  other callers send calls over them then, as MRCE's stations count down
  their own timeouts (`DxStn.pas:210-238`), whether they call on their own
  timeouts or answer your message. As built, a turn is planned with those
  calls in it when it goes on the air: each caller in it waits from the
  end of its own last send, one that heard your message and does not
  answer from the end of your message, and one whose wait runs out before
  the turn is over calls in it then, which can bring in the next. So a
  timeout never takes a turn off the air, and no character already
  sounding is cut and sent again. A timeout that would send its
  caller away sends nothing, and comes once the turn is over. What a call
  on its timeout costs (a try, or a silence for the station you work) is
  spent once it begins to reach you: a key that cuts the turn before then
  drops the call, and it costs nothing, as everyone's wait starts afresh
  after your message; a pause or failed sound that cuts it before then
  drops it too.
- **Patience** while calling is `3 + RndRayleigh(3)` tries, rounded
  (`DxOper.pas:496-498`). MRCE's `RndRayleigh(m)` is `m·√(−ln u₁ − ln u₂)`
  (`RndFunc.pas:55-58`), not a Rayleigh draw of mean `m`, so it gives
  4–14 tries with a mean near 7, and only 2.6% of callers give up after 4,
  which keeps callers from vanishing just after they call (MRCE issue
  #200, `DxOper.pas:486-493`). Each message a caller answers while it
  calls (CQ, TU, `?`, a partial, B4, NIL) costs a try too, as MRCE's
  `DecPatience` (`DxOper.pas:709, 825`), so CQ after CQ moves the
  frequency on to someone else. TU and B4 first give it fresh patience, as
  MRCE's `SetState(osNeedQso)` does (`DxOper.pas:751-753, 776-777`), so a
  caller passed over QSO after QSO stays while you run. The first partial
  it answers while it calls tops its patience up to five tries before the
  try, as MRCE's near match puts a station in a QSO with full patience
  (`FULL_PATIENCE`, `DxOper.pas:496-498, 736`), though here it stays out
  of the QSO (4.7), so asking for the station you heard never sends it
  away; a partial after that costs a try as any answer does. A pile-up
  caller that waited out your QSO with someone else comes back with fresh
  patience, as MRCE's `osNeedPrevEnd` does: to your CQ or NIL with no try
  spent on it (`DxOper.pas:708, 719`), and to a `?` with the Call field
  empty before the try the `?` costs (`DxOper.pas:793`).
- **The station you work stays** while you keep keying. Once it has your
  exchange, or you sent its call wrong, it leaves after 3 silent timeouts
  in a row, as MRCE's Caller Stays sends such a station away once its
  patience is spent (`DxOper.pas:319-335`); MRCE's leaves on its fourth,
  its patience set afresh as its state moved, and Dust's on its third, a
  choice of its own. As built (third pile-up audit), one still
  waiting for your exchange (`NeedNr`) asks for it on its first two
  timeouts, then stops asking but stays on the frequency, silent, with no
  timeout running, and takes your exchange whenever it comes, as MRCE's
  `osNeedNr` station does once its patience is spent, which `DecPatience`
  never sends away: a learner slow to send it again after `NR?` loses no
  QSO for it. Anything you send it brings it back to asking.
- **QRS** (phase 2, habits `easy`). After 3 incomplete tries (4 for a
  skilled caller) a caller slows by 12–20% by skill, 2% more from the
  fourth (`DxOper.pas:343-375`). As built (2c), as MRCE's QRS After a Few
  Incomplete Calls: an incomplete try is a slip, a piece or a partial of
  its call that reaches it (`mcAlmost`), or a `?` with no call in the
  message while it waits for its call put right or for your TU
  (`DxOper.pas:741, 790-803`). An `NR?` counts as a `?` does: Dust's own
  choice, as Dust's `NR?` asks as `?` does, where MRCE counts only the `?`
  (it has no case for `msgNrQm` there). A caller that answered your
  partial while it called counts as waiting for its call put right, as
  MRCE's near match puts it in `osNeedCallNr`. Its QSO moving on counts the tries
  afresh, but for the call put right still to come (`osNeedCallNr` or
  `osNeedEnd` to `osNeedCall`), as MRCE's `SetState` does. Once the tries
  reach 3, or 4 for skill 3, while its QSO is on, it sends 20%, 16% or
  12% slower as its skill is 1, 2 or 3, and 2% slower still from the
  fourth try, so a skilled caller slows by 14%: characters and spacing
  alike, never below the Speed card's lowest speed (5 WPM, MRCE's floor
  too, `DxStn.pas:166-168`), and only in its own sends; a send already
  sounding goes on at the speed it began at. Back on track, its call
  heard right before your exchange, it sends at its own speed; its call
  put right after your exchange, it keeps the slower speed for the
  exchange it sends with it, as MRCE's. A pile-up caller left to wait out
  your QSO with someone else keeps its speed and its tries, as MRCE sets
  `osNeedPrevEnd` without `SetState` (`DxOper.pas:744-748`): a partial or
  a slip that brings it back finds it as slow as it was, its tries counted
  afresh, as `SetState(osNeedCallNr)` does; your CQ, TU, B4, NIL, a `?`
  with the Call field empty, or its exact call bring it back at its own
  speed (`SetState` to `osNeedQso` or `osNeedNr`). A dupe calling again
  starts at its own speed. Each of its groups is credited at the slowest
  a send of it began to reach you at: its call at its slowest call, its
  exchange at its slowest exchange, so an exchange heard only at its own
  speed is credited at that, and a slow reply you keyed over before it
  began credits nothing (9.1). As built (4a), callers do so under easy
  habits only, a courtesy to a learner; under real and rough habits they
  keep their speed, as contesters do. As built with adaptive callers'
  speed (2026-10-22), a caller slows from the speed it came with, drawn
  from your copy speed while the callers adapt, and the belief never
  touches it once it calls, so the two never pull against each other
  (§6.6).
- **Calling twice.** 10% of pile-up callers send their call twice; single
  callers never do (`DxOper.pas:500-502`). As built, a caller that calls
  twice sends its call twice where the contest's answer has it once
  (`K5XYZ K5XYZ`); Simple QSO's answer sends it twice already.
- **Careless operators** (phase 4, habits `rough`), as MRCE's LIDs
  (`DxStn.pas:73`, `Station.pas:509-521`, `DxOper.pas:651-671, 879-886`).
  As built (4a): one caller in ten, in a contest whose callers send a
  serial, fumbles a digit of the first serial it sends, its last digit
  from 2 to 7 or else the one before it, one more or one less, then sends
  `EEEEE` and the serial right (`TT8EEEEE TT7`); what it sent, for the
  copy and the check, is the serial right, and it sends it right ever
  after. One operator in about twenty (MRCE's 0.88 − 5/6) sends its
  exchange with its call put right before it has yours, each time it puts
  the call right (`DE K5XYZ 5NN 214` after a slip, a piece or a partial of
  its call): a correction carrying its exchange, as `NeedCall`'s is, its
  exchange a copy once it begins to reach you. One still calling takes a
  partial it fits best as its call sent wrong, as MRCE's near match puts
  it in `osNeedCallNr` (`DxOper.pas:736`), so it is in a QSO with you, and
  your TU completes it, as MRCE's does once that exchange is out
  (`DxOper.pas:784-786`). The exchange it sends for yours after is owed,
  no repeat you asked for (4.6). And a caller takes a slip of its call for
  its own one time in 25 (MRCE's 4%), a call of four characters or more:
  it answers as if you had sent its call, so your log holds the slip and
  the check shows the call busted with its own (`busted call, was K5XYZ`),
  as MRCE's `CALL` does. MRCE's early exchange is one word of call (`K5XYZ
  5NN 214`); Dust's keeps a correction's two words, so where its exchange
  starts is read alike. MRCE's other LID habits (refusing its right call
  one time in a hundred, a report other than 599 now and then, no extra
  patience on garbage) are not built.

| You send | A caller still calling | The station you work |
|---|---|---|
| CQ or TU | calls; once its call has begun to reach you, that is a call you asked for again (9.1) | after TU: `Done`. After CQ: `Done` if it has already sent its exchange (`NeedEnd`); before that the QSO fails and it leaves (`DxOper.pas:705-711`) |
| `<call> <exch>`, exact | falls silent | sends its exchange |
| `<call> <exch>`, close (4.7) | falls silent | corrects you: `DE K5XYZ` or `K5XYZ K5XYZ`, each operator keeping its form; with your exchange in hand, five operators in six send their own after it (`DE K5XYZ 5NN 214`), as MRCE's `osNeedCall` (`DxOper.pas:851-867`) |
| `<call> <exch>`, no match | calls again after its timeout; in a pile-up it waits for that QSO to end, as MRCE's `osNeedPrevEnd` never sends (`DxOper.pas:227-228`) | in a pile-up, the QSO is over for it (`DxOper.pas:744-748`) and you work it no more, whether another station takes the call or none does: waiting for your exchange, it calls again and waits for the other QSO to end; its exchange sent, it is `Done` |
| `?`, `AGN`, `NR?` | repeats its call, unless it heard you work someone else since it last called and the Call field is not empty, as MRCE's `osNeedPrevEnd` (`DxOper.pas:744-748, 793`) | re-sends what it owes: call or exchange. After someone else's call, neither answers |
| partial `K5?` | only the best matches answer; your exchange after a partial it answered, in that message or a later one, is for it: it takes it and puts its call right (`NeedCall`), as MRCE's `osNeedCallNr` (`DxOper.pas:734-742, 768-776`) | sends its call, as MRCE's `mcAlmost` (`DxOper.pas:734-742`): `NeedNr` → `NeedCallNr`, `NeedEnd` → `NeedCall` |
| B4, NIL (phase 2) | calls again at once, the QSO you were in over: B4 with fresh patience, then a try; a NIL a try too, but for a caller back from waiting out the QSO, which comes back with fresh patience instead (`DxOper.pas:716-724, 751-753`) | leaves, and Enter starts afresh (4.4): only the station they are for, not one a B4 after another call names, nor one that heard you work someone else; one you sent a slip of its call (`NeedCall`, `NeedCallNr`) takes a B4 for the slip's and puts its call right, as MRCE's (`DxOper.pas:755`) |

From phase 2, in pile-ups only:

- **Collisions.** A caller still sending when your message starts does not
  hear it and keeps sending over you, except CQ, TU and NIL, which are heard
  from a fragment (`DxStn.pas:240-253`). As built (5a), without full
  break-in you do not hear it either: the receiver is muted while you send
  (§5.3).
- **Missed exchanges.** 10% of the time a caller misses your exchange and
  sends `NR?` or `AGN` (`DxOper.pas:762-770, 844-849`).

As built (2a): a caller still sending at your key goes on from the character
sounding then, sent again whole and, with full break-in, heard over your
message (without it, as it ships from 5a, the receiver is muted while you
send and nothing of it reaches you, §5.3), its start charged already, and
the ranking of the call you send counts it (4.7). One that never heard you
and is done before you are heard only your message's tail: it calls again
0.1–0.6 s after you, a try spent, as MRCE's callers do on hearing garbage
(`DxStn.pas:249-253`), but only after it takes two more tries, up to four,
as MRCE's `MorePatience` (`DxOper.pas:412-427, 805-808`), so being keyed
over sends no caller away. One still sending as you end waits from the end
of its send. A CQ, TU or NIL is heard from a fragment: the caller goes on
over it all the same, and answers once you and it are both done. Esc takes
the rest back with the message, but what of it had reached you stays heard
(5.2). A missed exchange is not taken, so N stays false and Enter sends the
call and exchange again, though the call that went with it is the call's
copy, as first sent with your exchange (9.1); a caller that answered your
partial and then missed your exchange puts its call right meanwhile
(`NeedCallNr`). Your call alone (F4) brings a caller still calling back as a
CQ does, unless it heard you start a QSO with someone else; a single
caller's call brought so is a `?` by another key, as a CQ's is (4.6). In a
pile-up, no call brought again by CQ, TU or F4 is charged.

A pile-up caller that hears you start a QSO with someone else waits for it
to end, as MRCE's `osNeedPrevEnd`: it has no timeout, so it neither calls
over that QSO nor spends a try on waiting, and a `?` with the Call field
typed is not for it; your CQ, TU, B4 or NIL, its own call, or a `?` with
the Call field empty brings it back, all but its own call with fresh
patience. So once the station you work has gone, nobody calls until you
CQ, and Enter's hint says so (7), and with a call the fields still hold
for it, Enter calls CQ in its place (4.4). So too for the call of a caller
that gave up before you worked it, typed late: sent, it would go to
nobody and leave the rest waiting on that QSO, so Enter clears it while
they call, and calls CQ once nobody does; a partial or a slip of it goes
out as typed, and once nobody calls, Enter calls CQ in its place (as
built, third browser test, 4.4). The station
you work hearing another call is over its QSO as the table says. A single
caller calls again on its timeout, and stays in its QSO when you send
another call, kinder to a learner.

A single caller you talk over stops and hears you, kinder to a learner.

A caller hears each of your messages whole, in order, when you key it, and
its reply is decided then (5.1); Esc takes back what had not gone out whole
(5.2). A key during a turn sends the rest of your message again, which the
callers heard when it was keyed, so they hear only the new message: an
exchange keyed then, after someone else's call, goes to the station you
work. A station that hears its call alone (F5) waits for your exchange and
asks for it with `NR?` or `AGN`, as MRCE's `NeedNr` does (in Simple QSO,
`AGN` alone: the contest's row says what its callers ask with). The patience timer
runs to the first waiting caller's timeout; each caller draws its own, and
keeps it till it sends or hears you again.

### 4.4 Enter Sends Message

ESM follows MRCE's `ProcessEnter` (`Main.pas:1162-1224`). **C**: the Call
field equals the call last sent. **N**: the station you work took your
exchange; one sent with a call nobody took for its own is owed still, and
one taken only by a station you do not work (another a tie reached too,
or the station just logged, with F2 after Shift+Enter) is no QSO of
Enter's. In a pile-up N holds only while the Call field could be the call
last sent: a call that cannot be is another caller's, which has had no
exchange from you, so Enter sends it its call and your exchange (`Send
W7ABC + exch`), where MRCE would send `W7ABC ?` and the caller answer
`NR?` for as long as Enter is pressed. Once the station you work has
answered the call sent with its exchange, or by asking for yours, with
no correction, that call is its own, and any other whole call is another
caller's (`QsoState`'s `call_taken`, set as the answer begins to reach
you, which the learner hears too). Every key first counts what of the
turn on the air has reached you, so Enter decides on what you have heard
however long other callers go on after the answer, and the screen's label,
hint and corrected border follow each send of the station you work as it
ends (4.2). Before then a slip of the call sent,
or what a partial asked for, may be the call put right, and N holds, as
in MRCE: a near call that is a real caller (K5XYA while K5XYZ's answer
has not come) is sent `K5XYA ?` first, and asks for your exchange. A
single caller stays in its QSO whatever call you send, and there N is
MRCE's.
**V**: the call is whole (`parse_callsign` accepts it), with no `?`. **E**:
the Exchange field is Empty, Missing (some fields, not all), Invalid (a
token that cannot be read, or a value off its list) or Complete (11.7). MRCE's Q
(RST typed) is always true, because RST is prefilled; in Simple QSO the RST
is part of E.

| Situation | Enter sends | Key label |
|---|---|---|
| Call empty | CQ | `CQ` |
| your TU without logging (F3) went to this call, V, E Complete | TU, and logs that QSO, as MRCE's C and N still hold after F3 | `TU & log` |
| the call, without `?`, is one the fields could still hold for a station that has gone (as built, third pile-up audit), or the call of any station gone, heard from it, that no station on the frequency would take (third browser test); or C, and the call sent, a partial or a slip, fits a station gone and none on the frequency, with nobody calling (third browser test) | with nobody calling, CQ; with someone calling, nothing, but for a partial, which asks as below; either way the fields clear | `CQ`, or `Wipe` |
| C, V, and the station corrected that call, unasked or in answer to `?` | nothing; focus to Call, as a `?` would only bring the correction again | `Fix call` |
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
leaving, on its timeouts or sent away by what you send (B4, NIL, a CQ
before its exchange, or its last try spent). As built (third pile-up
audit), every call the Call field could still hold for it is then one
Enter no longer sends (`QsoState`'s `gone`): the field as last keyed for
it, a partial (`K5?`, `K5X?`) too, the call it took, and its own call
once that has reached you, as you may have put the field right since
without a key, after its correction or a partial it answered; it has
left, and its copy is closed, so that tells nothing a score could still
use. Nobody would answer one, and in a pile-up the rest wait on that QSO
in silence, where `Send K5XYZ + exch` or `Ask K5?` would only go on into
it. While nobody calls, Enter calls CQ in its place, clears the fields
for the next station, and its hint says why ("K5XYZ has gone: Enter calls
CQ", or "The station has gone" for a partial). Once someone calls, after
a CQ that brought a caller or a B4 the rest of the pile-up called again
on, a CQ would go over them, in single style a `?` by another key (4.6):
Enter sends nothing and clears the fields for the call you hear ("K5XYZ
has gone: Enter clears the fields"), but for a partial, which asks the
callers as typed. The calls stay through a CQ or a TU sent meanwhile, and
through the same call sent again (B4 pressed again, or F5), until another
call goes out or the Call field is emptied. The station leaving is said
as it goes, by the hint's name for it ("K5XYZ has gone", or "The station
has gone" for a partial): on its timeouts, and as built (third browser
test) on its last try spent, as a caller that had only answered your
partial spends it on your next `?`. Sent away by your B4, NIL or CQ, it
leaves by your own doing, and nothing says that. As built (third browser
test), Enter does the same with the call of every station that has gone
once its call had reached you, a caller that gave up before you worked
it among them, while no station still on the frequency would take it for
its own (`QsoState`'s `heard_gone`). In a pile-up of five left to call,
the browser test typed the call of one that had just given up: it went
out with the exchange to nobody, the callers still calling fell silent
to wait on that QSO, and Enter offered the call again for as long as it
was pressed. Enter does with such a call as above, whatever QSO it is
in, and as the fields clear the screen reader hears the hint's words,
"W7ST has gone", as nothing said this station leaving. These calls stay
through any call sent and an emptied field, as the station has left for
good, until a station calling takes the call again, a new caller drawn
with it: then it goes out as ever, as does any call that fits a caller
still calling, a slip of its call included. Only the call as heard is
one of them: a partial or a slip of it names no station, and goes out as
typed, as a call that fits nobody there does, the rest of a pile-up
waiting on it. Once it has gone out to nobody and nobody calls, the
silence has told what the hint would (`QsoState`'s `sent_fits_gone`: the
call last sent fits a station gone, and none on the frequency), so Enter
on it calls CQ, the fields cleared, and the hint and the screen reader
say "The station has gone", naming none. A single caller that gave up is
the same: nobody calls, so Enter calls CQ in place of its call at once,
and a partial of it goes out once, into the silence, before Enter calls
CQ in its place. A call that fits no station heard goes out as ever. Nor
is a station logged and off the frequency one that has gone: its call is
in the log, the B4 tag and button show for it (4.7), and Enter sends it
as typed, the rest of a pile-up waiting on that QSO until B4 or a CQ
ends it. Taken for gone, it would be said to have gone though you logged
it, and Enter on the call your TU without logging went to, its exchange
not copied yet, would clear a QSO waiting to be logged. F3 keeps only
the call it went to, so Enter or `.` on that call sends the TU alone and
logs the QSO it closed, as MRCE's TU and log does while that call still
counts as sent, and any other call starts afresh. Its call sent in
answer to your partial, or to a piece of its call, is no correction: you
asked for it. A station that corrects you again after you sent its call
right never heard that call, sent over its correction in a pile-up: Enter's
QSO goes back to the call it last heard, still corrected, so Enter sends
the call again (`K5XYZ ?`) rather than wait on `Fix call` for a fix
already made. "They corrected you: fix the call, then Enter" is said once
for the call sent: the same correction again, on the station's timeout or
over a fix it never heard, is no news. A call corrected after your
exchange sends `K5XYZ TU` and logs; most stations send their exchange
with the correction (4.3), so one key puts the call right and logs. A
partial always goes out again as
typed, alone. Where MRCE refuses an invalid exchange
(`Main.pas:1210-1216`), Dust also sends nothing: the fault is your
typing. When Enter does not log and V holds, focus moves to Exchange, as
MRCE's `MustAdvance` (`Main.pas:1222-1223`); after logging it returns to
Call. `esm(&qso, &fields, &rules, side) -> Esm { message, label, hint, focus, corrected }`
is pure: the machine runs it on the key, the screen on its own field text.
It takes your side because ARRL DX's exchange depends on it, as
`parse_exchange` does. When rows combine, the label names each part:
`Send K5XYZ + ?`, `Send K5XYZ, TU & log`, `Send exch, TU & log`.

**Going on to another station.** In a pile-up you go on to another caller
by sending its call; the station you worked hears it and is over its QSO
(4.3). That is a QSO of its own for Enter: C is the new call, nothing is
corrected or taken, and N is whether the new station took your exchange,
not the old one. A call that fits nobody leaves you working nobody. The
station you leave keeps the Call field as you last read it while you
worked it: the call typed for the next station is never its copy, and a
key that sends nothing reads no other caller's call into it (9.1).

### 4.5 Keys and buttons

| Action | Desktop | Phone | Phase |
|---|---|---|---|
| ESM | Enter | the big key, labelled as in 4.4 | 1 |
| CQ | F1 | `CQ` | 1 |
| Again: `?` (4.6) | F7 | `? Again` | 1 |
| Stop sending; wipe the fields | Esc or Alt+S; Alt+W | —; `Wipe` | 1 |
| Call ⇄ Exchange | Space: on to Exchange from Call, back to Call from Exchange (as built after operator feedback, below); Tab as on any page | `␣ Field`, or a tap | 1 |
| Speed ±2 WPM; pause; end run | PgUp/PgDn; Alt+P; Alt+E | − / +; `Pause`; `End run` | 1 |
| Your exchange; TU without logging (as MRCE) | F2; F3 | — | 1d |
| His call; his call + exchange | F5; `;` (Insert also works) | — | 1d |
| TU and log; log without sending | `.` `,` `+`; Shift/Ctrl/Alt+Enter | (ESM) | 1d |
| Your call, B4, NIL, NR? (AGN in Simple QSO) | F4, F6, F8, F12 | `B4` on a dupe | 2 |

- F11 stays fullscreen (`app.rs:140-147`), so Wipe is Alt+W (Ctrl+W closes
  a tab), and `[` `]` stay the paddles (`paddle.rs:113-125`). Web
  fullscreen takes Esc, so the run calls `navigator.keyboard.lock(['Escape'])`
  where it exists, and Alt+S always stops. Insert is the screen readers'
  modifier key, so `;` is the documented key.
- During a run, the run's keys lose their browser defaults. One listener
  on the document hears them wherever the focus is, nowhere included, on
  every build: a script the run screen puts on the page, as the
  visibility listener is (6.4), which keeps each key from the browser and
  tells the run through `dioxus.send` (`src/ui/contest_keys.rs`). A key is
  known by where it sits (`KeyS`, as Alt+S types ß on a Mac); a key held
  down is told once; Ctrl and the Command key make a shortcut of any other
  key, and Shift of any key but a typed character (below), so those are
  passed by, Enter held with Shift, Ctrl or Alt aside. F1–F10 and F12 join
  the list as their keys ship.
  Alt+P pauses, and on a paused run carries on. As built (2026-10-23),
  Enter in the Call and Exchange fields, which the fields hear rather
  than the listener, is told once when held down too: it heard every
  repeat, about 30 a second, each keying a message more. MorseRunner's
  `FormKeyDown` acts on every repeat (`Main.pas:950`); Dust's keys never
  have.
- Every key has a button. The key bar's "Enter · TU & log" chip names what
  Enter sends, and F3 reads "F3 TU (no log)". After Enter come F1 CQ, F2
  Exch, F3 TU (no log), F5 His, F7 ? Again, `;` His + exch, `.` TU & log,
  Shift+Enter Log, Alt+W Wipe and Esc Stop; as built after operator
  feedback, Space comes straight after Enter (below). The buttons Enter
  would press now are lit (its CQ is F1's, their call F5's, your exchange
  F2's, `?` F7's, its TU `.`'s), and `.` and Shift+Enter wait, disabled,
  until the call is whole and the exchange complete (`loggable`). On a
  desktop window 760 px wide or more the bar took two rows, Enter to F5
  and F7 to Esc; with F4 My call, F6 B4, F8 NIL and F12 NR? (2a) it takes
  three, Enter to F4, F5 to `.`, and Shift+Enter to Esc, and three from
  640 px, none alone on a row; narrower, Enter takes a row of its own and
  the other keys two or three even columns. With Space after Enter
  (operator feedback) it takes three rows from 726 px, none alone: Enter
  to F3, F4 to `;`, and `.` to Esc. Narrower it is a grid: Enter spans
  every column but the last, Space in it, and the other fourteen keys fill
  three or four even columns (four from about 622 px, three from about
  480 px); under three columns, where Enter would have too little room
  for the call it names, Enter and Space take a row each, Space as a space
  bar, and the fourteen fill seven rows of two (a container query on the
  bar's width). The 16th key left Esc alone on a fourth row from 640 to
  about 724 px until the grid took that band too. F12 names the contest's
  own word for its query: `AGN` in Simple QSO, whose exchange has no
  number. While the call typed was logged clean, B4 is offered in the B4
  tag's copper wash, not lit, for Enter never sends it. Where a finger
  types, only CQ, ? Again and Wipe stay beside the keypad, and B4 on a
  dupe, at the row's end, so the keys a finger knows never move.
- `;` `.` `,` `+` are known by the character they type, Shift held or not
  (`+` takes Shift on many a keyboard, `;` on others), and never typed into
  a field, as in MorseRunner; one typed by a paddle key (`+` sits on `]` in
  German layouts) stays the paddle's. Enter held with Shift, Ctrl or Alt
  is no Enter in the fields; the listener hears it as log without sending.
  With the Command key, which the listener passes by, it is Enter still.
- Paddles key into the focused field (`paddle.rs:287-299`) and never
  transmit; phase 5 adds a field space after a word gap. As built, the
  keyer ends a word once six dits of quiet follow a letter's last element,
  a dit short of a word space so one sent on time always ends it, as it
  ends the letter once two have, the split between a letter's one and the
  three between letters (`Keyer`, its gaps in `keyer/clock.rs`; the same
  for a straight key and the paddles), and the run takes the word's end
  as Space, by the Space key's rule (`space_goes_to`): on from Call to
  Exchange, and from Exchange back to Call where the exchange has one item
  to type or the items typed read whole, else the space before the next
  item. So a call and its exchange keyed on the paddles land in their own
  fields. It is the whole of a word space, not the split between a letter
  space and a word space, so a hesitation between the letters of a call
  never moves the cursor out of it; one pause presses Space once, however
  long it lasts; and a key pressed in it, Enter or any message key, ends
  the word with no Space after it. So does anything done by hand: Space
  from the keyboard, the key bar or `␣ Field`, any key of the keypad,
  typing, or a click, a tap or Tab into the other field
  (`ContestEntry::finish_letter`), which would otherwise have the word
  space move the cursor a second time, back to the call from an empty
  exchange; a letter keyed in full and still waiting out its letter space
  lands first, in the field it was keyed for. A group's box and the
  practice pad take no word space, as a group has no spaces
  (`PaddleKeys::use_fields`, `src/ui/paddle.rs`).
- **Space** (as built after operator feedback, 2026-10-22) toggles between
  the fields as MorseRunner's and N1MM's space bar does, so going back to
  fix a call is one key. In Call it moves on to Exchange, as a call has no
  spaces. In Exchange it goes back to Call where the contest's exchange has
  one item to type (WPX, CQ WW, ARRL DX from either side), whatever is
  typed; where it has more (the Sprint's NR NAME ST, Field Day's class and
  section, Simple QSO's report and name, which §11.7's parser reads from
  one field), once the items typed read whole (E Complete), or the field
  is empty or already ends in a space; otherwise it types the space before
  the next item (`space_to_call`, `contest/esm.rs`), so no space it types
  is one the reading would drop. Moving, it types nothing, and the cursor
  lands at the end of the field it goes to with nothing selected, so ⌫
  takes the call's last character at once; every move of the run's cursor
  does so (`focus_end_script`, `src/ui/widgets.rs`), and the app's tests
  see the script that moved it. The keypad's `␣ Field` follows the same
  rule, from one place (`space_goes_to`, `src/ui/contest.rs`), as the key
  bar's Space and the hint do, and as built (5b) the paddles' word space.
  The key bar shows Space on a keyboard, desk-only, straight after Enter's
  chip, saying what it does now: "→ Call" from Exchange where it goes
  back, "→ Exch" from Call, named as a place so it never reads as a
  message key beside F2 Exch, and "Next item" where it types the space
  before the exchange's next item; its `aria-label` says the same in
  words, and it keeps one width whatever it says, so no key changes rows
  as an exchange is typed. It is lit, as Enter's own keys are, where it
  takes the cursor back to a call to put right, as Enter's `Fix call`
  does; a click does what the key does, the move or the space. With the
  station's correction to put right, the hint reads "They corrected you:
  fix the call, then Enter", which a phone's line holds whole, naming the
  Enter key it has, lit as `Fix call ↵`; where Space would go back to the
  call, a wider screen adds ". Space goes back to it" (`Esm::hint_for`).
  The cursor is in Exchange after your exchange went out with the call,
  which is when a correction comes, but a Sprint's or Field Day's
  correction can carry the exchange, part of which you may have typed:
  there Space types the space, and neither the hint nor the key says it
  goes back. Tab and Shift+Tab move as on any page. Before, Space in
  Exchange typed a space, which the user found confusing beside
  MorseRunner.

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
it: `?`, `NR?`, your exchange again (F2) or its call again (F5). One it
sends with its call put right (4.3) costs it only when your `?` or `NR?`
drew it: a partial or a slip asked for the call, and the exchange came
with the correction unasked. Nor does the exchange your own first draws
from a careless operator that sent its exchange early (as built, 4a): it
owes that one for yours. A single caller's call brought again by CQ
or TU, once it had begun to reach you, is a `?` by another key, but for
the TU of a key that logs: after your TU without logging (F3), Enter or
`.` on that call sends the TU again and logs, as MRCE's does, and the
caller F3's TU brought calls again on it, which you keyed to log, not to
hear it again, so it costs only time (as built, 2c; the browser test had
it charged, `?×1`). Re-sends you did not ask for (timeouts, calling
twice, corrections, pile-up calls before you picked the station, a send a
hidden page or a failed sound cut short and sent again after Resume, an
answer to a message of yours Esc cut short) cost only time (9.1).

### 4.7 Partials, AGN, wrong calls, dupes

- **Partial calls.** `?` stands for one character, a trailing `?` for any
  tail, matched unanchored as MRCE's regex (`DxOper.pas:546-590`). Without
  `?`, edit distance decides: 0 is exact; up to (length − 1)/2, or the
  pattern inside the call, is close (`DxOper.pas:597-631`). A close piece
  that is no call itself (`K5`, `5XYZ`) asks for the rest as a partial
  does, and is not put right as a slip is; a close slip that is no call
  (`KXYZ`, its digit dropped) asks so too, but fits with the characters of
  the call it gets right, as MRCE ranks it by edit distance, so it goes to
  the nearest call (K5XYZ, one edit, not K5XYA, two); a call with no
  character but `?` is the `?` of F7, nobody's call. Only the best share of
  matched characters answers; the single caller always does. As built,
  every station on the frequency is ranked, those still sending too, as
  MRCE's `FindBestMatches` ranks them (`StnColl.pas:171-194`), so a call
  keyed over its own station is no near call's: its own call first, then a
  slip, then a partial or a piece. Within a kind the station you work comes
  first, so a partial or a slip that fits it is for it (a partial while you
  work a station brings its call, 4.3), though a caller's own call still
  beats it; then the fewer edits for the call's length the better, or the
  larger the share of the station's call the characters cover (`K5?` is two
  of K5AB's four, two of K5XYZ's five). Those that tie at the top answer,
  and to the rest it is someone else's call. In a pile-up, a station that
  only answered your partial is no QSO yet: several may, and you work one
  (9.1). Until it hears its call from you or takes your exchange it is not
  the station you work: a call logged as its own is checked as a NIL, a NIL
  is not charged to it for another's call, and a partial to it leaves the
  QSO your TU without logging closed to log (4.8).
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
  already in the log or on frequency is drawn again, up to 8 times. As
  built (2a), dupes come in both run styles of a contest that scores (not
  Simple QSO, which is no contest and has no dupe rule), from the stations
  logged with a QSO (not a NIL) and done, but not one on the frequency at
  your key or closed by its TU, which does not call straight back: the same
  station, its exchange as it sent it but for the serial (11.6), its copy
  kept in the QSO it was logged in, so nothing that comes of the dupe owns
  groups or is a lost copy. B4 (F6, and the phone's `B4` on a dupe) sends
  the call typed first when it has not gone out (`K5XYZ QSO B4`), as TU and
  log does: a dupe that has not heard its call would not know the B4 was
  meant for it, and no other station takes it for its own. As built
  (third pile-up audit), so it does when the call went out but the
  station you work never took it: keyed over a dupe still calling, which
  never heard it, the B4 alone would be the rest of the pile-up's cue to
  call again, the dupe among them, with fresh patience, every time. It
  leaves, Enter starts afresh, and the rest of the pile-up calls again
  (4.3). A call already logged, typed again while its station is off the
  frequency, goes out as typed, B4 then telling the rest that QSO is over
  (4.4). A
  slip that is close to a caller's call goes to that caller, which puts its
  call right and stays, as MRCE's does.

### 4.8 Logging and the check

A QSO is logged on ESM's TU row, on `.` `,` `+`, or on Shift+Enter (V and
E Complete). The machine records the times, the logged and true values, the
caller's speed and sends, and your serial; the fields clear and the serial
goes up. A logged QSO is final. Its check is shown when the station is done
(`Contest.pas:855-895`): once your TU has reached you, and the sign-off it
brought where the contest has one (Simple QSO's `73 EE`), however long the
pile-up calls on after it; or once the turn carrying your TU is off the
air, however it leaves, ended, cut by Esc or Pause, replaced by your next
key, or at the run's end. A key while the TU still sounds sends it again
(5.2), and the check waits for that turn. Until then the row shows `…`.
A QSO logged without sending is checked at once, so nothing is `…` at the
end, after
what of the turn on the air has reached you counts. It is checked against
the station you work (in a pile-up, not one that only answered your
partial) when the call logged is its own or close to it (4.7),
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
(`mixer/mod.rs:111-117`, `web/mod.rs:233-240`), and desktop UI timers fire
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
`mixer/send.rs:185-201`). `others` are the replies, delayed and never cut,
so `plan_transmission`, which cuts at `wanted`'s end (`plan.rs:58-72`), is
not used. A turn of callers only has a `wanted` with no tones. Only
patience, the countdown, the resume timer and the run end use UI timers,
all seconds long, and the heard timer, which only brings Enter, the
screen and the log's checks up to what the turn has played (4.2), so it
may fire late. The app gains `start_planned` beside `start_send`
(`audio/mod.rs:81-90`) and `Lease::send_planned` beside `Lease::send`
(`player.rs:289-314`), keeping leases, stall detection and retries (§12);
as built (5a), both take the moment a continuation is to start
(`Turn::starts_at`), which every player honours (5.2), and the stall budget
waits that moment out. What the turn says goes to the player as
`Turn::transcript()`, a `Transmission` of your message with each reply as a
station calling across it after its delay: the real players play only the
plan, and the test recorder keeps both, with the `Qsb` the send was given.

### 5.2 Continuations, stops, failures, heard time

- **A key during a turn** does not cut your message, as MRCE queues a
  message keyed while you send (`MyStn.pas:119-136, 177-187`). The machine
  plans a continuation, a new turn: the rest of your message; then the new
  message; then the replies to it, which the callers decide on the new
  message alone, having heard the rest when it was keyed (4.3). As built
  (5a), it is a timed splice (`Splice`, `contest/machine.rs`): the turn on
  the air sends the character sounding at the key to its end, and the
  continuation takes over at the start of the first character of your
  message yet to begin, the moment it would have sounded, so no character
  goes out twice and none waits. A player plays ahead of the ear, by its
  lead and latency, and has to ride the send it takes over from down first
  (`SEND_RELEASE_SEC`, 8 ms, kept with the plans in cw-core for this), so
  the continuation takes over no sooner than both after the key: the
  machine learns the lead from each turn started at once, on its first
  try, as `heard_from` less the moment it was keyed (the player's lead,
  its latency, and the moment the start took), held to a second, past any
  lead and latency a player believes, so a start held up longer, as by a
  device opening, does not hold up every continuation after it; a
  character starting sooner after the key than that goes out with the turn
  on the air, which the player has already begun to play. With no
  character of it left to start so, the turn on the air finishes your
  message and the continuation takes over half a word space after its last
  character, so the send before is ridden down in silence, and the new
  message starts a word space after that character, as one message would
  have it, or as soon after as the player can start it; with your message
  gone out, it starts at once, as before. The turn says when
  (`Turn::starts_at`), and the player starts the new send then against the
  clock the old one plays on, which plays on to there and is ridden down
  by then, your message silent there, so the two never sound together:
  natively the mixer starts it that many samples into the send playing
  (`Mixer::start_at`), and so does the browser, which renders the same
  mixer ahead of its clock (`audio/stream.rs`). A moment already past, a retry's, starts it as soon as it can.
  Everything is decided when you key, so what the turn on the air plays up
  to the splice counts at the key, as planned. That is from the key to the
  splice: the rest of the character sounding then, any the player had
  already begun, and the gap after, or half a word space after your
  message's last character, which is up to about a second for a `0` at 25
  WPM, more at lower speeds or with a long lead. An Esc, a pause or the
  run's end in that time takes the turn off the air early but leaves it
  counted, so with full break-in a caller's send that would have reached
  you in it counts as heard though it never did, and without it, a reply
  starting in the word space before such a splice does too (nothing else
  of the callers is heard while you send, 5.3). Counting it as it plays
  would keep the turn on the air beside the next till the splice, for
  every count and timer, which is left for later. Before 5a, starting a
  continuation released the old send at once while the new one started a
  player's lead later, and the rest went on from the character sounding at
  the key, sent again whole, so one character went out twice. In a
  pile-up, callers still sounding at the splice carry on from their
  character at that moment, sent again whole, re-based to the
  continuation's start; with full break-in collisions stay audible, and
  without it they are muted under your message (5.3); a single caller
  stops and hears you. As built (2a), they carry on under a CQ, TU or NIL
  too, which they hear from a fragment, and answer once done (4.3); one
  carried on whose rest has not reached the ear yet, inside the player's
  lead, still goes on whole at the next key, as it began before. As built
  (2026-10-23), a caller's character, or QRM's, is sent again whole once:
  carried on again while the character a splice before sent again is
  still going, it goes on from where it is (`Segment::skip_sec`, which
  `plan_turn` starts its tones from, the tone sounding then going on to
  its end with a rise of its own). A splice falls between your
  characters, so keys over a caller sending slower than you used to land
  in its re-sent character every time and start it over, holding it on
  that character for as long as they came.
- **Only Esc cuts.** StopTx emits `StopAudio` (an 8 ms release,
  `SEND_RELEASE_SEC`, `timing/plan.rs`), which cuts a send a continuation
  was to take over from later too, on both players; a call or exchange not fully sent counts as not
  sent, as MRCE's Esc resets CallSent and NrSent (`Main.pas:884-891`). The
  callers heard the message when you keyed it (4.3), so Esc takes back what
  had not gone out whole: they are as they were before the message keyed
  last, then hear only its parts sent whole. A QSO the message itself lost
  (a CQ or TU keyed by mistake, or the next station's call while the QSO
  your TU without logging closed waits) is taken back with it, and so is a
  caller it brought, which never heard it whole; the groups of such a QSO
  reach the sampler only once it is final. A QSO logged since that key, or
  lost by a later key, is final, so then nothing is taken back; nor is a
  continuation's carried rest, heard at its own key. After Wipe, or a key
  with the Call field empty, C and N stay reset. As built (third pile-up
  audit), your message cut short, every caller that was listening, not
  sending still at the Esc nor waiting out your QSO with someone else,
  copies what it heard of the message as garbage, as MRCE's callers copy
  an aborted send (`MyStn.pas:108-116`: `AbortSend` makes it `msgGarbage`
  and fires `evMsgSent`), and answers it after its reply delay, 0.1–0.6 s
  (`DxOper.pas:805-817`, `DxStn.pas:240-275`): a caller takes more
  patience and spends a try, as when you key over it (4.3), and the
  station you work sends again what it owes, at no charge (4.6). So the
  pile-up comes straight back, as in MRCE, where it used to keep quiet for
  a whole reply timeout, thinking time MRCE never gives. With your message
  out whole before the Esc, the callers heard it: a reply it cut never
  sounded, and what they owe comes on their timeouts. What reached you
  since the key of the callers on the frequency then stays heard: a caller
  you keyed over that went on over your message keeps the end of a call
  or exchange heard whole, which its copy is timed from, and what it told
  Enter (that the station you work took the call, or corrected it) stands.
  Esc takes your message back from the callers, not their sends from you.
  A dupe the message brought back goes back as it was, whole, its old
  QSO's ends and voices with it, so a NIL after is never charged to the
  station you work for want of them.
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
- **Heard time.** `send_planned` tells `on_news` after each attempt starts,
  with no `Player` borrow held, and when a send was held up on its way
  (`SendNews`, `player.rs`). The runtime dispatches
  `TurnStarted{id, heard_from, output_from}` (`TurnHeldUp{id}` for the
  latter): the start plus the player's lead and
  latency estimate (web `schedule_lead_sec` and `heard_latency`,
  `wait.rs:124-132`, `web/mod.rs:122`; native `latency_frames`,
  `audio/native.rs:272-280`), and, as built (5a), for a continuation the
  wait for the moment asked, so `heard_from` is the splice the machine
  planned wherever the player could honour it; that of a turn started at
  once, on its first try, is how the machine learns the lead a splice
  waits out. A
  caller's segment counts as heard once it has reached the ear, unless the
  receiver was muted for it (5.3); your own count from the start, which is
  lenient. Each player's send says it as
  `PlaybackSignal::heard_after_sec`: the browser's lead and believed
  latency, read as the send is scheduled; natively, the latency the mixer's
  last callback found; nothing for the silent player and the recorder.

### 5.3 Your own transmission

You hear your messages from phase 1, at the side-tone centre
(`settings/ranges.rs:175-178`) and your run speed (6.6), with no Farnsworth
spacing and no fist variation.

- **Slice 1a** played it as `wanted` through the receiver at `VOLUME_MAX`.
  At the hardest noise it was only a few dB over the floor (2 dB SNR at
  full level, `band/calibration.rs:35-36`), and the AGC ducked a weak
  reply's first 0.3 s: an interim.
- **Slice 1b**, as built, plays it dry, as a rig's sidetone is played.
  `plan_turn` marks the plan `dry_wanted` (`PlannedTransmission`,
  `timing/plan.rs:36-45`) and plans your tones at `MONITOR_LEVEL`, half of
  full scale (`contest/turn.rs:16-33`), whatever the voice's volume, which
  could reach only 0.3 (`plan_morse_playback_at`). That is about 3 dB under
  the paddle's sidetone at the default volume floor (0.7,
  `player.rs:228-233`), 4.4 dB over the loudest caller before the AGC,
  about 11 dB over the hardest noise through the default filter (the
  loudest caller is 7 dB over it), and under the limiter's knee (0.7), so
  alone it is never bent and the band's crests on it are caught, not
  clipped. The worst band is the widest filter (2000 Hz) at the hardest
  noise and static: there it is only about 5 dB over the band's RMS, and
  the limiter catches the band's crests on it some 6% of the time. A
  steady tone still stands well clear of hiss two kilohertz wide, and a
  louder level would only bend more of it. Every turn's `wanted` is dry, a
  turn of callers alone included (it has no tones); `plan_transmission`
  never marks one, so group sessions and the band preview play as before.
  - The web player schedules it into `chain.floor_in`, around the filter,
    the fading and the AGC but through the limiter, as the paddle's
    sidetone goes: `ScheduledSend::schedule` takes `dry_into` beside
    `into` and gives your message a gain of its own, which a stop ramps
    down with the other (`web/send.rs:12-81`, `web/mod.rs:246-256`).
  - Natively one `SendVoice` holds your message apart from the stations
    it filters, unfaded, and gives both each sample (`SendSample { heard,
    dry }`, `mixer/send.rs:119-149`); `Mixer::render` adds `dry` after
    `heard * gain`, before the limiter, so the AGC never hears it
    (`mixer/mod.rs:187-191`). A stop releases both.
  - The callers keep their path through the filter, QSB-free, and the AGC.
    The send's length and `PlaybackWait` still follow the stretched
    `wanted`, and the background's reservation counts only the most of
    `plan.others` sounding at once (5.6). The silent player and the
    recorder are unchanged; the recorder
    keeps each plan, `dry_wanted` with it.
- **Your sending** (as built after operator feedback, 2026-10-22). Against
  weak callers the fixed level was painfully loud, the user said, and
  being able to turn your own sidetone down mattered most of the four
  changes. `MONITOR_LEVEL` is now the loudest your message plays, your
  sending at 0 dB; "Your sending" on the Tone and volume card
  (`mySendingDb`, §8) turns it down in decibels, as MorseRunner's
  self-monitor level is set (`MonLevel`, -60 to 0 dB, saved as
  `SelfMonVolume`, `Ini.pas:282, 461, 556`), to off at -60 dB.
  `plan_turn` plans your tones at `ContestSettings::sending_level()`, so
  both players play it as they played the monitor's level, the native
  mixer's dry sample and the web player's dry gain into `floor_in`, and
  the recorder keeps it with each plan: nothing in the players changed.
  Off, `wanted` has no tones of yours and lasts as long as ever, so the
  send, `PlaybackWait`, the turn's segments, the air line and the heard
  timer keep their time. It starts at -9 dB. By the figures above the
  loudest stands about 6 dB over an average caller before the AGC (4.4 dB
  over the loudest caller, and the U-shaped strength's middle, 0.85 of the
  volume range's top, is 1.4 dB under that), and the AGC ducks that caller
  3.5 to 5 dB more (`band/agc.rs`), so a caller is heard about 10 dB under
  it; at -9 dB your message is level with an average caller as you hear
  it, a decibel over, as a rig's sidetone set to the receiver's audio is,
  and still some 16 dB over the default band's noise through the default
  filter (2 dB over the hardest noise, which the loudest caller clears by
  7 dB before the AGC). Hear a QSO plays your message at it too.
- **The paddles' sidetone** does not follow it. In a run the paddles key
  letters into the fields and never transmit (§4.5), so their sidetone is
  the sound of typing into the log, not your sending; it plays at a level
  of its own in every mode, as a rig's sidetone does, half the limiter's
  knee (0.35 of full scale, `SIDETONE_LEVEL`, `audio/tone.rs`), turned
  down with the volume alone, and a setting of Contest mode's would make
  the same paddle sound different in another mode. Following it would
  also cost keying its sidetone: with your sending off the paddles would
  key in silence, and few can send without hearing it. It once followed
  the station strength floor, which set how far the stations stand over
  the noise, not how loud anything plays; at the default floor (0.7) the
  paddles sounded about 12 dB over your messages. At its own level, 0.35
  of full scale at full volume, they sound about 3 dB under your messages
  at the default -3 dB (0.50), some 4 dB under an average caller as heard
  (0.57) and 5 dB under the loudest station the receiver plays. A contester who keys the log with
  paddles hears that gap; one who types, as at 235 QSOs an hour, does
  not. Should keying with the paddles transmit one day (§14, later),
  their sidetone in a run would be your sending, and should follow it
  then.
- **The receiver gate** (as built, 5a). A rig without full break-in mutes
  its receiver while it transmits, as MorseRunner's does unless its QSK is
  on (`Contest.pas:806-834`; `Qsk`, `Ini.pas:251`, off as it ships).
  Without full break-in, `plan_turn` marks the receiver muted from the
  turn's start to the end of your message
  (`PlannedTransmission::muted_until`, from `Turn::muted_until`, the one
  place the rule is written), your sending off or not, as a rig is muted
  while it transmits whether you monitor it or not: the gate keys off that
  span of the dry plan, not off its tones, which your sending off leaves
  none of. A turn of callers alone mutes nothing. Both players mute the
  receiver's output after its AGC, the band, its audio-stage floor and
  every station alike, and never your message nor the paddle's sidetone,
  which come in past it. Natively `Mixer::render` multiplies what the
  receiver gives by a gate that moves a straight line at a time, over
  `GATE_RAMP_SEC` (5 ms), toward shut while a send says you are sending
  (`SendSample::keyed`) and back once none does, so it never clicks; a
  send handing over to a continuation goes on saying so to the handover,
  its release and all, as your message runs on into the next, so the gate
  stays shut across a splice part way through your message. At a splice in
  the word space after your message's last character the gate opens in
  that space, as the machine hears a reply that starts in it, and shuts
  again from the takeover. A second key taking over from a continuation at
  its own start goes in its place exactly, a moment of clock rounding past
  it being no moment at all (`SendVoice::samples_until`), so neither
  leaves the gate a sample in which nobody is sending. The browser plays
  the very same mixer, rendered ahead of its clock and made again from the
  moment of any change (`audio/stream.rs`), so its gate is this one. The
  AGC runs on under the gate, so the band comes back as it would have
  been. A stop opens it from where it stands.
  - What callers sending across your message sound like with it shut:
    nothing, as in MorseRunner. So nothing they send while you send counts
    as heard: a caller's send any of which falls under your message (the
    rest of a call keyed over, or a call on its timeout that comes before
    your message ends) is no send heard at all (`Turn::muted`), as a call
    heard in part is no call heard. It starts and ends no copy, charges no
    repeat, credits no speed, tells Enter nothing (no answer taken, no
    correction), shows no text with Show copy, and the operators never
    hear it; RX counts a caller only from your message's end, and the heard
    timer looks only at sends that reach you. What it costs its caller is
    spent all the same, a try on its timeout among it, as it was sent
    (`Counted::Muted`). Collisions are as they were for the callers: one
    still sending at your key never hears you and goes on over you (4.3),
    now unheard, and calls again a moment after you, which is heard. Every
    reply comes after your message, a tenth of a second at least, so the
    gate, open again 5 ms after your message ends, takes none of one.
  - With full break-in (`contestQsk`, §8) nothing is muted: the band and a
    caller sending across your message are heard through it, as every run
    was before 5a.

The turn plays from a copy of the run settings with `qsb_enabled = false`:
QSB is attached per send (`Qsb::of`, `audio/native.rs:244`) and `BandKey`
has no QSB field (`band/key.rs:19-49`), so the band never rebuilds. Callers
fade on their own from phase 2 (5.5), in their tones' envelopes, so the
send still carries no QSB.

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

As built (2a), `place_contest_caller` (`contest/place.rs`) places one
caller at a time, as each joins, against the callers already on the
frequency, and `filter_capacity` gives the Band card its count. Placed
at random, callers would wedge the reach shut well short of its capacity,
so a pitch is offered at random up to eight times and taken only if it
leaves room for the rest, else the lowest place left, which always does:
capacity callers always fit. The reach is the spread either side of the
centre within the −15 dB edges. Strength is drawn U-shaped for single
callers too, `sin(π(u − ½))` across the volume range; a dupe calling
again keeps its strength and is placed afresh.

### 5.5 Per-caller QSB (phase 2)

The envelope curves carry the gain: `build_envelope_curve` bakes
`target_gain` in (`plan.rs:121-124`); the native player reads `target_gain`
only for a curve with fewer than two points (`mixer/send.rs:64-67`), and
the browser plays that same mixer.

Each caller gets the band's `Qsb` model at its own time offset, following
the Band card's QSB switch. `plan_turn` bakes the fade into each tone's
curve point by point (`ENVELOPE_SAMPLE_RATE`, 4,800 a second), reading the
fade a millisecond apart along the tone (`FADE_STEP_SEC`) with straight
lines between, as the player reads a send's fading; one `Arc` per tone, and
`target_gain` scaled to match. As built (2a), single callers
fade too. The machine holds `Qsb::of` the run's band settings, none with
the switch off, and each caller draws a start point of its own up to an
hour into the fade; a segment carries `Fade { qsb, from_sec }`, the
caller's point at the segment's start (its start point, plus the turn's
key time, plus the segment's start), and `plan_turn` reads only that, as
the runtime plays from settings with the switch off. A segment of yours
is never faded, whatever it carries. The QSB control runs
0.03–1.5 Hz (`settings/mod.rs:47-48`) with components up to 2.3× that
(`band/qsb.rs`), so a fade held per tone would step audibly. No player
changes: training's pile-ups fade the same way in the player, each station
on a path of its own (`fade_from_sec`, drawn as a caller's is).

As built (5a), flutter: under the Band card's flutter (`contestFlutter`,
§8, in Contest mode), three callers in ten flutter, as MorseRunner's
flutter has them (`DxStn.pas:97-99`): a caller's signal rises and falls at
a rate drawn for it from 3 to 33 Hz, from half to three quarters of the way
down (6 to 12 dB), so a fluttering caller is rough but never lost. MorseRunner's flutter only quickens a station's
fading, full depth, and acts only with its QSB on; here it is a flutter of
its own, the band's wandering multipath fading at the flutter's rate
(`Qsb::flutter`), over the band's fading where that is on and alone where it
is off, so the switch does what it says. Each caller draws its flutter once,
from a stream forked off its own generator (`draw_flutter`,
`contest/caller.rs`), so a run under a seed is reproducible and one with
flutter off draws as it always has. A segment's fade carries it (`Fade {
qsb, flutter, from_sec }`), read along the caller's own path, so it carries
through continuations and a dupe calling again keeps it; `plan_turn` lays it
into the tones' envelopes as it lays the fading, and your message never
flutters, nor QRM. The fade is read a millisecond apart, thirteen readings
to a cycle of the fastest path of the fastest flutter (2.3 × 33 Hz), which
cuts its corners by a fifth of a decibel at most, and both players play the
envelopes as they are, so no player changes for it: the browser plays the
native mixer, rendered ahead of its clock.

### 5.6 QRM and the background

The band runs as now (`band/mod.rs:9-30`); its traffic already sends contest
phrases, more as `stationsMax` rises (`activity.rs:194-195, 1281-1305`), and
at one station it is empty (`activity.rs:43-50`). While callers sound, the
background gives up slots counted from the most of `plan.others` sounding
at once (`PlannedTransmission::others_at_once`; `mixer/mod.rs:162-166`;
web `Reservation`), so your message takes none. As built, a turn plans
each call a caller sends on its timeout as one more of `others`, so
`others.len()` counted every call planned into it; the count at once is
for the whole send, not over time, so the band comes back only once the
turn is over.
On-frequency QRM comes with Caller habits `rough` (phase 4): a scripted
station at 30–50 WPM sending `QRL?`, a long CQ or `<you> QSY QSY`
(`Contest.pas:783`, `QrmStn.pas`).

As built (4a, `contest/qrm.rs`), a station starts up on average every
232 s of run time, each start an exponential wait after the one before
whether or not a station still sends, as MRCE starts one in 0.0002 of its
blocks of 512 samples at 11,025 Hz, whatever is on the air: 15.5 an hour,
46.5 messages. It sends one to five messages, four seconds apart give or
take two, and goes. One that starts up while another still sends waits for
it to finish and a gap more, where MRCE's would send over it, so no two
are ever heard at once; of those that start up while the frequency is
quiet, only the last is still there as the next turn starts, so a long
silence brings one station, not a queue of them. Its first is `QRL?` one
time in seven, `QRL? QRL?` two, a long CQ three and your call with `QSY
QSY` one; the rest are long CQs. It sends at 30 to 49 WPM, its spacing as
its characters; its call is one of the contest's callers' at your tier,
its pitch near yours and within what the filter passes plainly (Gaussian,
out to ±300 Hz), its strength anywhere in the volume range, and it fades
on a path of its own. The long CQ is the rules row's `long_cq`: `CQ CQ
TEST K5ABC K5ABC TEST` in WPX, CQ WW and ARRL DX (`Contest.pas:491`),
`CQ CQ FD … FD` in Field Day (`ArrlFd.pas:444`), and Dust's
`CQ CQ NA … NA` in the Sprint and `CQ CQ CQ DE … K` in Simple QSO. The
players hold one send, so QRM is heard inside a turn, as segments of its
own (`Sender::Qrm`) through the receiver as a caller's are: a message due
during a turn goes in at its time, one due while the frequency was quiet
as the next turn starts, and none is cut, so a long CQ late in a turn runs
the turn on, the callers' timeouts planned with it. A key while it sends
carries its rest into the next turn from the character sounding, as a
pile-up caller's, as it never stops for you, and nothing more of QRM goes
until a gap after that rest, which may run on past where its message was
planned to end (a start retried, or a lead): no station sends over itself.
Esc, a pause or a failed sound drops it. It never hears you nor answers,
is in no QSO and is no station: RX counts callers only
(`AirView::calling`), Enter and its hint never hear of it, the operators
and the soak leave it (as a listener knows it by its pitch and speed), the
copies and timing of §9.1 never look at it, and Hear a QSO shows it as it
plays, a muted line of its own headed QRM that begins no QSO; with Show
copy on (as built, 4b) the run's RX line shows its text as it shows the
callers', muted and after a QRM tag, as Hear a QSO keeps it, and still
counts it among no callers (§7). Its draws
come from a stream of its own, so a run under easier habits draws as it
always has.

### 5.7 Native, web and silent

The silent player completes a turn at its planned length; the app's tests
use the fake recorder (`src/testing.rs:131-133`). As built (5a), both start
a continuation at its moment too: the silent player waits it out, and the
recorder keeps how long after its start the send was to be heard
(`Started::heard_in`) and plays from then. On a hidden page the web
clock parks and a send waits (`wait.rs:88-107`); the run pauses first.

### 5.8 Later

Phase 5 adds a receiver gate after the AGC while you send, as a non-QSK rig
mutes (`Contest.pas:806-834`). Since 1b both players hold your message
apart from what the receiver hears (`SendSample::dry`, in the mixer both
play), so the gate keys off the dry plan and
closes only the receiver's output; until then the band runs on under your
message, as with full break-in. Phase 5 also adds per-station gain for
flutter, and a player that starts a send at a given time, so a continuation
splices in a gap. All three landed as slice 5a: the gate, with full
break-in a choice (5.3, §8), off by default as in MorseRunner; flutter
(5.5); and the timed splice (5.2), at the start of the next character of
your message that the player can still take over at.

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
- **Speed.** Callers' speeds are drawn from the Speed card's ranges:
  evenly, through `resolve_station` (`timing/station.rs:33-47`), or, as
  built with adaptive callers' speed (2026-10-22), from your copy speed
  within them while the callers adapt (below). As built after
  operator feedback (2026-10-22), you send at your speed, "Your speed" on
  the Speed card in Contest mode (`myWpm`, §8), a setting of its own as
  MorseRunner's `Wpm` is (`Ini.pas:237`, saved); until you set one (0) you
  send at the middle of the callers' character range, as before, so
  nothing changed until it is set. − / + moves it by 2 WPM, MRCE's step
  (`Ini.pas:238`), from the whole speed nearest it, within the Speed
  card's bounds (5–80 WPM), for your next message; the run bar and the
  Speed card round it the same way, half away from zero, so a middle of
  16.5 reads 17 on both and a press moves what is shown by 2. It is kept: the
  run writes it to the settings as it moves, stored once the run screen
  is left, as nothing is stored while a run is on, so the next run sends
  at it. The callers keep the Speed card's speeds, those already calling
  (`DxOper.pas:235-262`) and those who arrive later alike, but for slowing
  down for you (QRS, 4.3). − / + used to shift later callers too, for the
  run only; with your speed kept from run to run, a shift of theirs that
  lasted only the run would leave your speed kept and theirs put back, or
  need the card's ranges rewritten mid-run. MorseRunner's callers follow
  your speed (`DxOper.pas:235-250`), as they have no range of their own.
  The stored run's `my_wpm` is the mean of the speeds your messages went
  out at, what you sent.
- **Callers' speed from your copy** (as built, phase 5's adaptive caller
  speed brought forward and decided with the user on 2026-10-22, §15.2).
  The user asked for callers that speed up as you copy their calls clean,
  as an option on by default. MorseRunner's callers move only by your keys
  (its HST mode sends them at your speed), and RufzXP steps its speed up on
  each right call and down on a miss; Dust draws each new caller's speed
  from the Bayesian copy-speed belief group training's adaptive speed uses
  (`SpeedBelief`, `speed.rs`), so the speed moves smoothly as the posterior
  does, and a few lucky calls move it less than many. While "Adapt
  callers' speed to me" is on (`contestAdaptSpeed`, §8), the call source
  draws each newcomer's character speed and spacing with `draw_speed`
  along the Speed card's line from its slowest corner to its fastest
  (`SpeedPath`), its spacing kept within the Effective speed range
  wherever the characters are as fast as its slowest, as an even draw
  keeps it, so every caller comes within the card's ranges, but for QRS
  (below), aiming for the share of characters right that "Aim for"
  asks (94% at Balanced), the target group training's adaptive speed
  aims for: one setting for every adaptive feature, shown on the Speed
  card beside the switch while the callers adapt, as it is in Callsigns
  mode while speed adapts (`aim_section`). A contest's own fixed target
  was the other choice; it would have been a second, hidden notion of how
  hard to push, and the copy-speed card reads its series at the Aim's
  share. Off, the callers' speeds are drawn evenly from the ranges, as
  before: the source draws nothing, so the run is the very run it always
  was. One speed set leaves nothing to adapt, and the switch says so.
  - The belief starts each run from the Contest copy-speed series in the
    history (§10): the calls and numbers (NR, ZN) of every contest run, by
    its log's layout (`ContestRun::group_copies`), faded one session as a
    run starts (`SpeedBelief::for_session`, as a group session's). An
    assisted run counts as any run does, as group training's rules count
    every session of its set whatever it was sent with (blind to `sends`),
    and its copies are what you had by ear when each text showed (§9.1).
  - It learns within the run from every call and number copy as its QSO
    becomes final, when the call source is taught its groups
    (`ContestMachine::teach`, `CallSource::learn_speed`): the very groups,
    at the very speeds, the stored run keeps, so the belief at the run's
    end is the one the next run starts from, but for that fade. A field
    from a short list (a name, a state, a section, a class, a power, a
    report) is half guessed and says nothing of speed. A QSO a message on
    the air lost waits, as Esc may take it back.
  - Each call and number counts whole (`whole_evidence`,
    `SpeedBelief::learn_whole`), live and in the history alike: a
    character lost off its end is a miss. A group session's lost end is
    its length's to answer for, which the copy span learns from
    (`speed_evidence`), but a call or a number is as long as it is and no
    span learns from a run, so a busted call (K5XY for K5XYZ), a number
    short a digit, the partial a lost QSO keeps (K5) and a Show copy text
    frozen while your typing lagged all slow the callers. Counted as
    group training counts them, they read as clean copies, and faster
    callers, losing more ends, would push the speed up further. A field
    left empty still says nothing: the run keeps no word of whether it was
    heard at all, and a caller gone before its exchange reached you leaves
    one as a send too fast to follow does.
  - Asking again never slows the callers: a copy counts as it ends, at
    the slowest send of it that began to reach you (§9.1), as the series
    counts it, blind to sends as group training's is, so a call got after
    a partial or a `?` counts as one got first time. What asking costs
    shows in the rate and the points; a call asked for and still busted or
    lost counts against the speed (above). Under easy habits QRS adds to
    this, as the copy is credited at the slowed speed. Counting a repeat
    as a partial miss would need the record to keep what each send gave
    you, which it does not.
  - A caller too fast to work at all, one you never answer, leaves no
    copy when it gives up, as the stored run's rules say (§9.1), so it
    teaches the belief nothing. Early on, while the belief is wide, a few
    such callers can come before ones you can partly copy narrow it, and
    an early run may stall for a while; the Speed card's Max bounds them.
    Counting them would need the run's log to keep callers it never
    worked, so that the history replays them; a known limit, as built.
  - Callers already calling keep their speed; only a newcomer's is drawn,
    and a dupe calling again is the same operator at its old speed. The
    rest of a caller comes from its own generator as ever, its speed alone
    set over the one `resolve_station` drew (`Caller::send_at`).
  - QRS under easy habits (§4.3) still slows a caller down for you, from
    the speed it came with. The two never pull against each other: the
    belief picks only a newcomer's speed and never touches a caller once
    it calls, and QRS slows one caller within its QSO and is gone with it;
    the copy is credited at the slowest send of it that began to reach you
    (§9.1), which is the speed the belief learns it at.
  - − / + move only your speed (`myWpm`), and the draw keeps within the
    Speed card's ranges, so they leave it alone.
  - The speed is drawn from the call source's generator, seeded for each
    run as its calls are, so a seed and the keys pressed give one run.
  - Hear a QSO (§2) draws its callers so too, from a sampler built from
    the history as a run's is, which learns within it what the perfect
    operator copies, as it does the characters, and is let go of after.
    Listen's stream of groups is no caller, and keeps to an even draw in
    Contest mode (§2).

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
│ Copy their number, then TU & log.                                │
│ TX ▸ K5XYZ 5NN T1T                               RX ▮ 1 calling  │
│ ┌ CALL ────────────────────┐  ┌ EXCHANGE · NR ────────────────┐  │
│ │ K5XYZ▌                   │  │ 214                           │  │
│ └──────────────────────────┘  └───────────────────────────────┘  │
│ Reads as: NR 214                                                 │
│ [Enter · TU & log] [Space → Exch] [F1 CQ] [F2 Exch]              │
│ [F3 TU (no log)] [F5 His] [F7 ? Again] [; His + exch]            │
│ [. TU & log] [Shift+Enter Log] [Alt+W Wipe] [Esc Stop]           │
├──────────────────────────────────────────────────────────────────┤
│ Log                                              9 QSOs · 8 clean│
│  9  07:31  W6XYZ     214        24 WPM          clean            │
│  8  06:58  K5ABD     76     ?×1 busted call, was K5ABC           │
├──────────────────────────────────────────────────────────────────┤
│ [ ⏸ Pause ]                          [ End run ]   [ Discard ]   │
└──────────────────────────────────────────────────────────────────┘
```

- **The hint line** is `esm()`'s `hint` ("Type the call you hear, then
  Enter"; with nobody calling, after a Simple QSO's TU or once the station
  has gone, the rest of a pile-up waiting on its QSO, "Enter calls CQ for
  the next station", from `QsoState`'s `nobody_calling`; as built, third
  pile-up audit, with a call the fields could still hold for a station
  that has gone, "K5XYZ has gone: Enter calls CQ", or once someone calls
  "K5XYZ has gone: Enter clears the fields", from `QsoState`'s `gone`, so
  the screen says what the screen reader's "K5XYZ has gone" says; as built,
  third browser test, the same for the call of any station gone that you
  heard and no station on the frequency would take, from `heard_gone`, and
  once nobody calls, "The station has gone: Enter calls CQ" for a partial
  or a slip sent to nobody that fits a station gone, from
  `sent_fits_gone`; as built after operator feedback, with the station's
  correction to put right, "They corrected you: fix the call, then
  Enter", and ". Space goes back to it" after it on a wider screen where
  Space would go back, `Esm::hint_for`, §4.5), one line, as
  group training's (`training.rs:566-585`), and in its type. The hints are
  short enough for a phone's line ("Copy their number, then TU & log").
  Before the first CQ it says the seconds to it ("Your first CQ goes out in
  3 s"), as a phone shows the log's place, where the countdown stands, only
  below the keypad.
- **The air line** shows only your own message, the part still being sent.
  RX counts the callers sending, QRM none of them (as built, 4a), and,
  without full break-in, none while the receiver is muted for your message
  (as built, 5a), but
  never shows what they send (Show copy, phase 4, is the exception). As
  built (4b), with Show copy on, RX takes a line of its own under TX and
  shows each station's text a moment after its send ends (a second,
  `SHOWN_AFTER_MS`, §9.1), the whole of it even where a key carried its
  rest into a turn of its own, in the order they showed, after the count
  of callers, ` · ` between them; QRM's is muted after a QRM tag, as Hear a
  QSO keeps it (§5.6). A turn's texts stay once it ends, and give way to the
  next turn's or to a pause. The line is a one-line slot cut short as every
  line is, and TX and RX each keep one height whatever they hold, the "—"
  of an empty line drawn at the line's own size, so the layout never moves
  as a turn comes and goes; at a phone's width, below 760 px of height, the
  keys and the air's two lines close up a little (`.run.shows-copy`), so a
  360×640 screen still holds the run down to the keypad's last row (to
  634 px, in every state of the two lines). It
  is not read out as it changes: a screen reader hears the run's events in
  its one polite region, and reads the line when asked. With Show copy off
  nothing changes on the run screen.
  Each says "—" when it has nothing, as "Reads as" does.
- **A correction** turns the Call border to `--warn` and fills the field,
  with a "corrected" tag by its label, so it never reads as the focus ring:
  "They corrected you: fix the call, then Enter." (`esm()` says so with
  `corrected`.)
- **The key bar's buttons** keep the cursor in its field, as the keypad's
  keys do: a click on CQ or + takes no focus, so the letters typed next and
  Enter still reach the fields. So do Discard and Keep it, and the header's
  theme button on the training screen; Keep it and the theme button hand
  the cursor back after a press, as the key bar's buttons do. Enter's chip
  keeps one width, cutting a long label short, so the keys after it never
  change rows.
- **The log** is newest first, six rows tall. A row shows `…` until its
  station is done, then a word ("clean", "busted call, was K5ABC"), since
  icons are `aria-hidden` (`widgets.rs:277`). `?×1` counts asked repeats.

**Phone, running** (keypad always shown). From header to keypad it fits one
screen (`styles.css:2606-2657`); below 760 px height the keys shrink and
the run's cards, and the lines in them, draw closer, so a 360×640 screen
holds it down to the keypad's last row, Enter included. Every line is a
fixed one-line slot with an ellipsis (as `.train-hint`,
`styles.css:1259-1267`), so the keypad never moves between turns.

As built, after a browser test of group training, the rule is written for
`.train`, which a session of groups and a run share, so a session draws
its cards closer the same way, and its groups with them, and a 360×640
screen holds it down to Next and ⌫ in every group mode, through the
countdown, a group sent more than once (its "Send 1/3" badge and "3× per
group" chip, beside adaptive speed's) and every hint: the last row ended
at 656 px of 640, and ends at 631. The run's values are as they were. A
long group answered keeps to its slot there as a short one does: what was
sent, which the top row spells out already, is cut short beside its mark,
and the two rows of its comparison stay a row each, laid on one set of
columns (`.comparison`, `widgets.rs:302`), so a copied character stays
under the one sent there however many were typed past the end. The
columns narrow to fit, 11 px for a group of 20 at 360 px, and stop at
10 px, which still holds a letter inside its box: past that, from 22
boxes at 360 px, the two rows scroll sideways together, the next box
showing at the card's edge. Before, what was sent wrapped onto a line of
its own and the comparison onto four rows, or more with characters typed
past the end, pushing the last row to 728 px, and to 776 px with 20 typed
past a group of 20. Wider or taller screens draw a session as before.

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

At a phone's width the run bar keeps the desktop's three lines: the status
and the clock, the bar, then the QSOs, the rate (without its word, and a
Simple QSO's pace without its word and "/QSO": "2.6 min") and − / +, whose
targets are larger than the keys they show; the rate or pace keeps whole,
the counts before it giving way first; a status too long
for its line is cut short, and an open run's clock drops its " · up to
60:00". Where the keypad shows, Pause, End run and Discard come under it in
one row and the log below them, its rows two lines each; the key bar drops
Enter's chip, which the keypad's Enter replaces, Stop, and the names of
keyboard keys.

At a phone's width an exchange of more than one field, a Sprint's
serial, name and state or a Simple QSO's report and name, takes three
parts of the fields' row to the call's two, its boxes' letters closed up,
so a whole one shows ("214 PATRICIA NY" at 360 px, beside "W1ABC/7") and
its label keeps its whole word, "Exchange · NR NAME ST"; each field's
label keeps to one line, so the two boxes stay level (2b). The air's
count of stations calling drops its word there ("RX ▮ 3"), leaving a
Sprint's exchange going out its line, and a hint too long for the line
leaves what Enter then does to a wider screen ("Copy their number, name
and state", and on the desktop ", then TU & log" after it, `Esm::hint_more`).
The desktop log's exchange column holds a Sprint's serial, name and state.

The keypad takes its rows as props, not the constants at
`training.rs:353-357`: the character
rows stay as in group training, only `,` and `.` leave row four, and the
last row is `␣ Field` (Space's rule, §4.5: Call to Exchange, and back
to Call from Exchange but where it separates an item) and the ESM
key, whose label and `aria-label` name what it sends, cut short before its
`↵` when long. Fields keep `inputmode="none"` (`training.rs:205`); a tapped
field takes focus and the paddles.

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
37", a full-width Resume and the log, with Discard and End run below, End
run no longer primary, so Resume is the one thing to press. The pause sign
is drawn as an icon, as `⏸` is missing from many a phone's fonts. Discard,
paused or not, asks once more: "Discard this run? Its QSOs are not kept",
with the confirming Discard first and Keep it last, where Discard stood,
so the second tap of a double tap keeps the run. As built (4b), Pause keeps
its place in that row while paused, inert (`disabled`) and reading Paused,
with End run beside it as before: the paused page is shorter, so the row
can come to rest under the finger that paused, and a double tap on Pause
used to end the run there, End run having slid into Pause's place. Resume
stays the one thing to press, in the paused card; Pause's place holds no
second Resume, so a double tap leaves the run paused.

**Results** (a contest branch of `ResultsView`, `src/ui/results.rs:59-167`;
the same view opens read-only from a History row, in the list's place, with
History at its top and Back at its foot, and no Run again):

```
┌────────────────────────────────────────────────┐
│  ╭─────╮   Run complete · CQ WPX · 10:00       │
│  │ 92% │   12 QSOs, 72 an hour. Two QSOs       │
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

The ring, captioned "CLEAN" as the group results' ring is "ACCURACY", and
named "92% of QSOs logged clean" for a screen reader, is the share logged
clean. The sentence counts QSOs logged: those you asked a repeat for (the
`?×n` the run showed, which the log keeps), each check that found something,
and any station worked and never logged. ACCURACY is
character accuracy, as Home and History show it; POINTS are Dust's copy
points (10); the banner ranks within the contest kind. Where what you
logged differs from what was sent, the log reads logged→sent (`13→31` is
13 logged, 31 sent; `K5ABD→K5ABC`), and a QSO never logged says so.
Simple QSO shows QSOs, minutes per QSO and accuracy per field, with no
contest score: the second tile reads "Per QSO 1.2 min", the sentence "8
QSOs, 1.2 minutes each.", and in the score's place an "Accuracy per field"
card gives the character accuracy of the calls and of each field over the
run, lost copies included (`ContestRun::accuracy_by_field`), with the
checks' line under it. Its run bar reads "Pace 1.2 min/QSO" once a QSO is
logged (`ContestView::minutes_per_qso`); which contests score is
`ContestKind::scored`. Field Day's score is its QSO points, nothing
multiplying them, so its Mults read `—` (as built, 3b;
`Scoring::multiplied`). In the results' log nothing is cut short: the call,
the exchange and the check wrap within their tracks, and on a phone the
exchange and the check each take a line under the call. A History row reads "2026-10-20 · Contest · CQ WPX · 11/12 QSOs · 72/h · 97% ·
496 pts", and a Simple QSO's "… · Simple QSO · 8/9 QSOs · 1.2 min/QSO · …".

**Settings, Contest chosen.** The Character set card (§2) shows the six
segments in a 3 + 3 grid, the contest pills under a "Contest" eyebrow, "You
send 5NN + serial · You copy NR" as one line, the dialogue (You `CQ DL1ABC
TEST` · They `K5XYZ` · You `K5XYZ 5NN TT1` · They `TU 5NN 214` · You `TU`,
with `<your call>` until the call is one `parse_callsign` reads), the tier
stepper and the run style. Your station follows it. The dialogue is built
from the rules row (`ContestRules::example_qso`): your CQ, exchange and TU
as sent, a placeholder caller from the countries that call you, and its
exchange as callers send it, each copied field's example
(`Field::example`). The callsign tier's examples ("What this tier sends")
are not shown: the dialogue shows a call. Session shape's countdown reads
"Countdown before the first CQ", and the Speed card says the callers come
at speeds drawn from its ranges; as built with adaptive callers' speed,
"Adapt callers' speed to me" stands in place of Adapt speed to me, with
"Aim for" under it while it adapts (§8).

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
| Character and effective speed ranges | the callers' speeds, drawn within them from your copy speed while the callers adapt (`contestAdaptSpeed`, on as it ships), evenly otherwise; you send at your speed, `myWpm`, or at their middle until you set one; −/+ moves yours and keeps it (as built after operator feedback; it used to shift both for the run) |
| `callsignLevel` | the callers' call shapes, shared with Callsigns mode |
| `stationsMin/Max` (`settings/mod.rs:414-417`) | Single: labelled "Band activity". Pile-up: "Callers at once", as today; as built its unit reads "callers" ("caller" held at one), and "Your filter fits 6 callers" follows it. Picking Pile-up while the range holds one station opens it to 1–3, as a pile-up of one is none and MRCE's starts busier than its single calls (`Ini.pas:261`); a range you have set is kept |
| `pileupSpreadHz` | Pile-up only: how far either side of the centre callers land; hidden for Single, where it does nothing |
| Session shape's first field | "Minutes per run" in place of "Groups per session" (`settings.rs:288-295`) |

**Hidden in Contest mode** (`app.rs:1606-1635`): Experience presets; the
group fields (groups per session, group size, adaptive size, Aim, sends,
answer timeout, pause after a group, lock typing); Pile-up level, which
needs a wanted station; Adapt speed to me, group training's switch, in
whose place the callers' own shows (as built, adaptive callers' speed,
below); Auto level; the alphabet controls. As built, the Aim shows on the
Speed card while the callers adapt, as it does in Callsigns mode while
speed adapts, and that card's reset puts it back: whenever the card shows
it before the reset or after it, as the reset turns the callers' switch
back on and so can bring the Aim onto the card (`reset_section`).

**Presets** (phase 2): First QSO, Club and Contester on the contest card.
As built after operator feedback (2026-10-25), each sets the callers'
speed and nothing else: the character range, and the spacing's with it as
below. A preset that wrote the tier, the run style, the habits, the
stations, the band and your speed put back whatever the learner had set
there, and the tier, the stations and the band are group training's too.
The learner sets those, and a preset leaves them as they are. The card
reads "Callers at 15–18 WPM", or on none "Custom · a preset sets how fast
the callers come, and nothing else", and reads Custom once the callers'
speed is edited, as Experience does; Experience in turn sets the group
size and the speed, and no longer the session's length. What follows is
the presets as they were built before that, kept for the record.

Each writes shared fields (speed ranges, tier, stations, QSB, QRN) and
the contest's run style and Caller habits (as built, 4a), and flutter (as
built, 5a), and reads
"Custom" once any is edited, as Experience does (`experience.rs:83,
98-110`, `settings.rs:135-177`). No level is stored.

As built (2c), `ContestPreset` (`settings/contest_preset.rs`) applies
through the calls the screens make, as `Experience` does, and the card is
on a preset exactly while every field it set still holds; a fading or
static switched off holds whatever its depth or level, which the band does
not hear then. It writes the run style too, and from 4a the callers'
habits, the fields it sets that are the contest's own. The pills sit under
the card's QSO, a line under them saying what the preset sets in the Band
card's words ("15–18 WPM · tier 1 calls · one caller at a time · easy
habits · no fading or static", each part kept whole where a narrow line
breaks, the habit as the Band card names it), or, on none, what a preset
would set ("Custom · a preset sets the callers' speed, calls and habits,
how many call, and the band"), as Experience's note does on none. They are
pills, not Experience's dropdown in a card head: they sit inside the
Character set card between the Contest and Run style pills, three short
names that fit on a phone's line, and the Run style pill a preset flips
shows at once what it set.

| Preset | Callers' speed | Tier | Run style | Habits | Stations | QSB depth | Flutter | QRN level |
|---|---|---|---|---|---|---|---|---|
| First QSO | 15–18 WPM | 1 | Single caller | easy | 1 | off | off | off |
| Club | 20–26 WPM | 3 | Pile-up | real | 1–3 | 0.35 | off | 0.25 |
| Contester | 28–36 WPM | 6 | Pile-up | rough | 2–5 | 0.5 | on | 0.4 |

First QSO is a beginner's first contest QSO: one caller at a time, a
little faster than a Koch learner's 15 WPM, with the plainest calls
(K5XY-shaped, tier 1), on an empty, quiet band. Club is a club operator's
few contests a year: everyday contest speed, calls from anywhere (two-letter
prefixes, tier 3), the small pile-up MRCE starts a pile-up at, and the band
as it ships. Contester is a contester's: callers at 28–36 WPM, every shape
of call with portables and stations abroad (tier 6), up to five at once,
and a band that fades deep and crackles. As built (4a), First QSO's
callers send plain numbers (easy), Club's numbers as contesters send
them (real), and Contester's add careless operators and QRM (rough), the
hardest preset taking the hardest habits. You send at the middle of the
speed range: as built after operator feedback, each preset puts your speed
back there (`myWpm` 0), so settings a preset left before your speed was a
setting are on it still, and a speed of your own afterwards reads Custom.
Callers space at the speed they key, as contest callers
do: the character range is written, and with the speeds tied the spacing
follows it; with Farnsworth spacing set, the same range goes in the
spacing's range too (the switch is left as it is), and a spacing changed
after reads Custom. As built with adaptive callers' speed, no preset
touches "Adapt callers' speed to me": a preset says how fast the callers
may come, and whether they are drawn within that from your copy is yours,
so turning it on or off keeps the preset. As built (5a), Contester's band
flutters too, the hardest band taking the fastest fading, its line reading
"deep fading and flutter, strong static", and First QSO and Club turn
flutter off. Full break-in is the rig's way of working, as your sending is
its monitor's level: no preset touches it, and turning it on or off keeps
the preset.

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
| `myState` | String | `""` | a state, DC, province, territory or `DX` (`PQ` read as `QC`, `NF` as `NL`); else `""`; `LB` joins with ARRL DX (as built, kept in any contest and sent as `NL` but in ARRL DX) | Station | 2 |
| `myZone` | u32 | 0 | 0..=40 (0 = from your call, and your state in North America) | Station | 3 |
| `myClass` | String | `1D` | `[1-9][0-9]?[A-F]`, else `1D` (as built, leading zeros dropped: `03A` is `3A`) | Station | 3 |
| `mySection` | String | `""` | a listed section, `YT`, or `DX`; else `""` (as built, `YT` kept and sent as `TER`) | Station | 3 |
| `myPower` | String | `100` | 1..=1500 (leading zeros dropped), `K`, `KW` or `1K` as written; else `100` | Station | 3 |
| `contestHabits` | `easy` `real` `rough` | `easy` | forgiving | Band conditions | 4 (as built, 4a) |
| `contestShowCopy` | bool | false | — | Session shape | 4 (as built, 4b) |
| `contestTuCall` | `every` `everyThird` `never` | `every` | forgiving; `third`, an older save's, reads as `every` | Character set | operator feedback; 2026-10-29 |
| `contestAdaptSpeed` | bool | true | — (read back, anything but true or false is on) | Speed | adaptive callers' speed |
| `myWpm` | u32 | 0 | 0 (the callers' middle) or 5..=80; read back, anything but a number is 0 | Speed | operator feedback |
| `mySendingDb` | i32 | -9 | -60..=0 (-60 is off); read back, anything but a number is -9 | Tone and volume | operator feedback |
| `contestFlutter` | bool | false | — (read back, anything but true or false is off) | Band conditions | 5 (as built, 5a) |
| `contestQsk` | bool | false | — (read back, anything but true or false is off) | Tone and volume | 5 (as built, 5a) |

- **Caller habits.** `easy`: plain digits, no LIDs. `real`: numbers as
  MRCE's callers send them (11.6). `rough`: LIDs and on-frequency QRM. As
  built (4a), the Band card shows them in Contest mode only, after the
  pile-up's spread, as "Caller habits" with Easy, Real and Rough and a
  sentence for each that follows the contest and your side, the numbers
  you copy cut as its callers cut them (`ContestRules::real_numbers`). In
  WPX: "Easy: plain digits, nobody slips, and callers slow down for you
  when you struggle. Real: numbers as contesters send them, T for 0, N for
  9 (TT7 for 7), now and then O for 0 and ENN for 5NN; nobody slows down
  for you. Rough: real, with careless callers who fumble a serial, send
  their exchange too soon or take a wrong call, and now and then a station
  on your frequency that is not calling you." CQ WW's example is `A4` for
  14, ARRL DX's from W/VE `ATT` for 100, Simple QSO's `57N` for 579; Field
  Day and the Sprint cut nothing, and say that Real plays as Easy but for
  nobody slowing down; a fumble is named only where callers send a serial.
  The card's note names the callers' habits in Contest mode, and under
  Rough habits a single caller's Band activity hint says QRM comes even at
  1. Its reset puts easy back. `clamp()` has nothing to bound: like the
  contest and its run style, the field holds only what it names.
- **Show copy** (as built, 4b). A switch on the Session shape card, in
  Contest mode only, after the minutes a run lasts and its countdown:
  "Show copy", with "Shows each caller's text on the run screen a moment
  after it is sent. A run with it on is marked assisted: it counts in every
  stat, but is greyed on the board and never placed there." Read back,
  anything but true or false (a save edited by hand) is off and costs the
  rest of the save nothing; its card's reset turns it off; `clamp()` has
  nothing to bound, as for the habits; no preset touches it.
- **Your call in TU** (as built after operator feedback, 2026-10-22; every
  QSO by default and a single caller's too since 2026-10-29). On the
  contest card under the run style, with one caller at a time or a
  pile-up, where the contest's TU leaves it to you
  (`ContestRules::tu_call_chosen`: WPX,
  CQ WW, ARRL DX and Field Day; not the Sprint, whose TU always signs by
  its rules, nor Simple QSO, whose TU greets the station by name and
  brings nobody): "Your call in TU", with Every QSO, Every third and
  Never, and a line saying what each does. The card's example QSO, your
  first, signs its TU as that QSO would: "TU DL1ABC" with Every QSO, and
  plain "TU" otherwise (`ContestRules::tu_adds_call`, which the run's TU
  follows too, through `Fill::tu_signed`). A single caller's TU signs as a
  pile-up's does, though MorseRunner's single calls never ID (its station
  ID is for its Pile-Up and WPX modes, `Contest.pas:471`): a run station
  signs its TU on the air either way. Where the control is not shown (the
  Sprint, Simple QSO) it keeps its value. Every third saves as
  `everyThird`; `third`, which every save wrote while every third was the
  default, chosen or not, reads as every QSO, once. Read back, a name
  this build does not know is every QSO; its card's reset puts every QSO
  back; no preset touches it.
- **Your speed** (as built after operator feedback). On the Speed card in
  Contest mode, after the callers' ranges: "Your speed", in whole WPM,
  showing the callers' middle until you set one, rounded as the run bar
  shows it (16.5 reads 17), with a line saying so and that the run
  screen's − and + move it and it is kept for the next run; once set, a
  line saying it is yours from run to run, and "Send at the callers'
  middle", which puts it back to 0. It reads its own values
  (`TrainingSettings::my_wpm`), so the card never redraws for them. The
  card's note reads "The callers' speeds, Farnsworth spacing, and yours"
  in Contest mode, and its line under the ranges "Callers come at speeds
  drawn from these ranges." Read back, a number is kept for the clamp to
  bound and anything else is 0. The card's reset puts it back to 0; a
  preset, which once did too, leaves it as it is (above).
- **Adapt callers' speed to me** (as built, adaptive callers' speed,
  2026-10-22). A switch on the Speed card in Contest mode, where group
  training's "Adapt speed to me" sits and in its place, after the line
  under the ranges ("Callers come at speeds drawn from these ranges.") and
  before your speed: "New callers come faster while you copy calls and
  numbers clean, slower when you miss them; those already calling keep
  their speed. Your copy speed is under Stats." With one speed set it
  says, as group training's does, "Give Character speed a range to use
  this: one speed leaves nothing to adapt.", naming Effective speed too
  ("Give Character speed or Effective speed a range…") only while the
  speeds are untied and its range shows. On, "Aim for" shows under it
  (§6.6). It is the contest's own (`contestAdaptSpeed`), on as it ships,
  so group training's switch (`adaptiveSpeed`, off as it ships) is left as
  it was, and neither steers the other's mode (`adaptive_speed_on` reads
  the mode's, for the sampler and the card alike). Read back, anything but
  true or false is on, as it ships, costing the rest of the save nothing;
  the Speed card's reset turns it back on, and puts the Aim back with it
  (above); `clamp()` has nothing to bound; no preset touches it.
- **Your sending** (as built after operator feedback). On the Tone and
  volume card, where the volume range lives, in Contest mode only, after
  the volume: a slider "Your sending" in 3 dB steps from Off to 0 dB,
  reading "−9 dB" in decibels as MorseRunner's self-monitor reads (the
  card's volume reads a gain, a station's share of full scale; a monitor's
  level is told from its loudest, as MorseRunner's is), with a short line:
  "Your own messages in a run, apart from the volume above: 0 dB is the
  loudest, −9 dB about level with an average caller, and Off silent, the
  run keeping its time", its −9 dB the default cw-core keeps. Read back, a
  number is kept for the clamp to bound and anything else is -9 dB. The
  card's reset puts -9 dB back; no preset touches it.
- **Full break-in (QSK)** (as built, 5a). On the Tone and volume card in
  Contest mode only, after your sending, as both say what you hear while
  you send: a switch "Full break-in (QSK)", with "Hear the band through
  your own messages, and a caller sending across one. Off, the receiver is
  muted while you send, as on a rig without it, and nothing sent meanwhile
  reaches you." Off as it ships, as MorseRunner's `Qsk` is (`Ini.pas:251`,
  saved), so the receiver is muted while you send (§5.3); on, the band is
  heard through your message, as every run was before. MorseRunner keeps
  it with its station's settings; Dust's Your station card holds your call
  and exchange, which are no rig's, and its reset would take them with it.
  Read back, anything but true or false is off, costing the rest of the
  save nothing; the card's reset turns it off; `clamp()` has nothing to
  bound; no preset touches it.
- **Flutter** (as built, 5a). On the Band card in Contest mode only, after
  the fading, beside which it plays: a switch "Flutter", with "Three callers in ten
  flutter, their signal rising and falling 3 to 33 times a second, as over
  a polar path, fading or not. Your own message never does." (§5.5). Off as
  it ships, as MorseRunner's `Flutter` is (`Ini.pas:265`); Contester turns
  it on and First QSO and Club off, and the Band card's note in Contest
  mode names it ("fading and flutter"). Read back, anything but true or
  false is off; its card's reset turns it off; `clamp()` has nothing to
  bound.
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
  contest fields: from 1c, a `myName` where the contest sends one ("Add
  your name to start a Simple QSO"). Your station shows the name field
  only where the contest sends a name, and Home, once the call reads, holds
  an inline name field as it holds the call's, and Start moves the focus to
  it. From 2b, a state where the contest sends one ("Add your state or
  province to start an NA Sprint", the contest's name kept on one line).
  Outside the US and Canada your state is DX, a simplification: the rules
  have a station outside North America send DX and one elsewhere in North
  America its country, and Dust has every station outside the US and
  Canada send DX. While `myState` is empty, a call Dust places outside the
  US and Canada sends `DX` (`ContestSettings::state_sent`), so a run needs
  no state from it; a call whose prefix Dust cannot place may be in them,
  and is asked for its state. As built, where a call is at home is read in
  `contest/data/places.rs` (`in_us_or_canada`), from every prefix the US
  gives Alaska and Hawaii and every prefix Canada issues, as the call table
  holds only some of them: Alaska and Hawaii count as in the US, as their
  stations send AK and HI in the Sprint, and Hawaii's prefixes with a digit
  other than 6 or 7 (KH2 is Guam) are outside it. Your station shows the
  state field only where the contest sends one, its line under it saying
  what it reads as ("Reads as Massachusetts", "Sends DX: in Dust every
  station outside the US and Canada sends DX") or that what was typed is no
  state ("MASS is not a state or province: type its code, as MA or ON, or
  DX"), however the field was left, Start and Listen on Home among the
  ways, and only until anything else of your station changes. Home asks for
  the state after the name, as it asks for the name. As built (3a), ARRL
  DX asks for a state only from the W/VE side, and CQ WW for your zone
  only where your call does not say it ("Add your CQ zone to start a CQ
  WW"), which Home says above Start, with a field of its own in the hero
  that Start takes the cursor to. Your power is always given. As built
  (3b), so is your class, `1D` until you give one: one typed with leading
  zeros keeps its number (`03A` is `3A`), and anything else is `1D`. Your
  section is the one you give, a listed section, `YT` (sent as `TER`) or
  `DX`, or else the one your call tells (`ContestSettings::section_sent`,
  §11.4); a call in the US's districts or Canada, or one Dust cannot
  place, needs it given ("Add your section to start a Field Day"), which
  Home asks for after the zone's place in the order. Your station
  says a power typed that is none ("MAX is not a power: …; 100 W is
  sent") until anything else of your station changes, its reset among
  them, as it says a state. Your zone follows your state too where you
  give one for a call in the US or Canada, in every contest, and the lines
  under your call and your zone then name it ("zone 3 in California").
  Under the call, Home and Your station say why a call typed does not
  read, in `call_fault`'s words (§2), and Your station, while the call is
  empty, what a run is missing, in the words of
  `ContestSettings::missing()`. Home says `missing()` above Start once a
  start is refused, unless the line under its call field says it already.
  A start refused another way in says it in a message.

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
parses each log once per load, its rows made again only when the history
changes, and a run's row and its results read their figures from one
`ContestRun::summary`.

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
    pub my_zone: u32,             //   (as built, 3a: kept for every run)
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
    pub l: Vec<String>, // the exchange you logged, a token a field: a NIL's, or where no group holds it
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub lc: String,     // the call logged, where the call group does not hold it
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub s: Vec<String>, // what the station sent of it, beside `l`, for the check
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sc: String,     // the station's call, where an earlier QSO holds its copy
    #[serde(default, skip_serializing_if = "is_zero")]
    pub a: u32,         // repeats you asked of the station and heard, as the live log's ?×n
}
```

Your callsign and exchange are not stored in a run: no score needs them.
Checks, dupes, points, multipliers and rates are worked out on read, as
`score` is. `a` is kept because the groups' `sends` count only the repeats
charged to the copy, and the results show the `?×n` the run showed. `v`
rises only for a change an older reader would misread; a key that later
phases add with a default does not raise it.

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
  hold what you logged and what was sent, for the check. As built (third
  pile-up audit), of a call put right with the exchange after it
  (`DE K5XYZ 5NN 214`), the exchange has begun once its first character
  has, not once the `DE` has, and so its speed and a repeat of it count
  from then (`exchange_at`, `contest/caller.rs`).
- **One station, one copy.** A station logged again (a dupe, or a log
  after a NIL charged to it) owns no groups: `sc` holds its call, `lc` the
  call logged, and `l` and `s` both exchanges. The soak checks that every
  station worked, logged or lost owns its groups in exactly one QSO.
- **A lost QSO** (`k: "lost"`): a station whose call you sent, or that
  answered your partial, whose QSO ended unlogged (wiped, gone, failed by
  your CQ, closed by your F3 and never logged, open at the end). In a
  pile-up, where several may answer a partial and you work one, only a
  station that heard its call from you, or took your exchange, has a QSO
  to lose; one that only answered your partial has none. One call
  group, then one per copied field, each a miss (received empty, 1 send,
  the token the station sent or would have sent), whether its exchange had
  reached you or not, as a QSO logged before its exchange came has. So a
  station costs the same groups however its QSO ends, and walking away
  once the exchanges are out, or the moment yours is, never raises
  accuracy or the board's points per group above finishing. The call's
  received = the call as first sent with your exchange, as a logged QSO
  keeps it, even with an exchange the station missed; never sent so, the
  Call field as last read while you worked it (a wipe reads it before
  clearing), without `?`; empty is a full miss. A key that sends another
  station its call reads the field only once the callers have heard it, so
  in a pile-up the station it leaves keeps what it had, and a key that
  logs reads it for the QSO it logs. In a pile-up a key that sends no call
  reads no field that holds another station's call, its own or one it
  took from you, as a copy of the station you work: it was typed for that
  station, so the station you work keeps what it had, and is named by it
  when it gives up. So no lost copy is a call another station took from
  you, which the soak checks as each copy is lost, against the stations
  there then. It is timed as the call was when you last sent it.
- **A NIL** (`k: "nil"`): a call group, and a group for each field as a
  logged QSO has them (as keyed once the station's exchange has begun to
  reach you, a miss before that), against the station that took the
  logged call from you for its own; or else the heard station whose call
  the logged one is or is close to, as the log would take it for a QSO
  with it (4.7), whoever you work now: a QSO logged for a station you
  left, a slip and all, is no copy of the one you work. Or else against
  the station you work (in a pile-up, not one that only answered your
  partial, nor one you left for a call that fits nobody), whose call the
  logged one is not close to, or that QSO would be logged; or else, in a
  run of single callers, the single caller: the one on the frequency, or
  else the newest heard, by when it was heard, as a dupe calls again at
  its old place, and never an older one; in a pile-up the newest caller
  is no likelier the station than any other, so a call like nobody's is
  charged to nobody. Received = the logged call. So a NIL owns the groups
  a QSO logged with that station would, copied no better, and never pays
  better than finishing: one logged on a caller before you work it copies
  none of its exchange, and one logged with a busted call once the
  exchange is in keeps the fields as you copied them. That station then
  has no lost copy as well, and a QSO logged with it later owns no
  groups. When the first station found already owns its groups (lost,
  logged, or charged a NIL before), or no station was heard at all, there
  is no group, and `lc` holds the call; no other station is charged, the
  station you work included. So no NIL's call group is a call another
  station took from you, which the soak checks as each is charged, as it
  checks each lost copy.
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
  `NR?`, or a CQ or TU that brings a single caller's call again, but for
  the TU of a key that logs, 4.6), alone or
  as its answer putting yours right, before you sent it with your
  exchange, and in a pile-up within the QSO it was asked in: once a QSO
  with another station is over, logged or left, a caller still calling
  forgets the repeats asked of it, as its next call is unasked and is the
  one copied, so a `?` to the whole pile-up is no charge on later QSOs; an
  exchange the station you work sent again at any key of yours (4.6), but
  one it sent with its call put right only at your `?` or `NR?`, as a
  partial or a slip asks for the call. A repeat counts once it has begun
  to reach you, even if you key over the rest of it, and only once an
  earlier send of the same had begun to reach you: a `?` keyed before
  anything of the station reached you asks for nothing again, and one
  keyed while its first exchange still sounds is a repeat. One Esc stops
  before it begins was never heard; one a pause or failed sound cuts
  before it begins is charged when it comes after Resume. A repeat counts
  only for what it could teach: a field's for fields empty, or read
  differently, after it; a call's when the Call field, without `?`, read
  otherwise at the key than the call finally copied.
- **`time_to_complete_ms`.** A call: from the end of its last send heard
  whole before the Call field's last edit, among the first and those a key
  of yours drew (CQ, TU, `?`, a partial), to that edit, at the key that
  sent it with your exchange; a call never sent so is timed as when you
  last sent it. Fields: likewise from the end of the last exchange heard
  whole before the field first read complete, among the first and those a
  key of yours drew, one sent with the call put right among them, shared
  by length; a repeat drawn after the field read complete changes nothing.
  A send on a caller's timeout restarts neither: waiting costs time, not
  sends. `Fields` carries both stamps; the 100 ms a character floor
  applies (`session.rs:26, 177`). Both ends are in run time, so a pause
  between the edit and the key neither shortens nor lengthens a copy. As
  built (5a), without full break-in a send that went out under your
  message, the receiver muted, was never heard (§5.3): it ends no copy's
  time and starts none, is no repeat, credits no speed, and Show copy never
  shows it.
- **Speeds.** Each group carries the caller's speeds to 0.1 WPM, as of
  the slowest a send of it began to reach you at, so a caller that slowed
  down for you (QRS, 4.3) credits the copy at the speed it may have been
  copied from: its call at the slowest call it sent, its exchange at the
  slowest exchange; the log row's WPM is the slower of the two, and the
  results list it the same, the slowest of the QSO's groups (as built,
  third pile-up audit; the results used to read the call's alone). A QSO
  that owns no groups, a station logged again, shows no speed in either:
  its copy, and the speed that is credited at, are the earlier QSO's. The
  session's are their mean, as `GroupSession::result` does
  (`session.rs:478-491`), never `my_wpm`. A call's and a number's
  speeds are what the copy-speed belief learns them at, live as their QSO
  is final and from the history after (§6.6). `digits_level` comes from the
  settings, `level` is the callsign tier, `score` is `points()`, and the
  tally and accuracy are worked out as for groups (`session.rs:458-521`).
- **Show copy** (as built, 4b). A station's text shows a second after its
  send ends (`SHOWN_AFTER_MS`), not the moment it ends: a learner types
  what it hears a beat behind it, and a copy cut off at the end of the send
  would always miss the character heard last, a mistake no ear made, which
  Letters, Mistakes and the sampler would all learn from. The call, and the
  exchange, of a station whose text first shows before you copy it (send
  the call with your exchange, or log the QSO) keep what the field held when
  it showed: later edits count only for the log. The showing is a send of
  its own, so the group's `sends` is 2 plus the repeats you asked for before
  it, its points halved as for a repeat, and its time is the floor, the copy
  done by then. A call or an exchange you sent or logged before its text
  showed was copied by ear and keeps that copy, with no send for the
  showing. The machine knows when each text shows; what the fields held
  then it reads at your next key from `Fields::earlier`, what each field
  held before every edit since the fields were last cleared, at a log, a
  wipe or the run's start (`Fields::text_at`): every edit is kept, so none
  made after a text showed can pass for what you had then. A copy made
  with no key since the text showed (a station lost on its timeouts, or at
  the run's end) keeps the field as last read at a key, as a lost copy
  always has. A Call field that held a call another station signs or took
  from you, or one a station gone left there (`QsoState::gone`), was typed
  for that one, whatever the style, and of this one you had nothing: the
  copy is empty, the showing a send. A call stays in the field past its
  QSO, one F3 thanked and not yet logged, or one gone until Enter clears
  it, as the next caller's text shows, and without this one typed call
  would be the copy of two stations, a mistake no ear made. A single
  caller, alone on the frequency, keeps any other call as you had it, but
  one you had typed for a station worked before; in a pile-up a call
  neither this station's nor close to it was typed for another caller. In
  a pile-up the copy is empty too when another station takes that call
  from you later, before the copy is made. A text that has not shown when
  the next turn goes out, the run pauses, or the sound fails, never shows,
  and is no copy's cut-off: the station's next send shows it. The log keeps what
  you logged: where a group holds the copy kept when its text showed and not
  the log, the record holds the log (`l` and `s`, `lc`), so the check and
  the contest score are worked out again as they were. The run is marked
  `contest:<kind>:a` and stored `assisted: true`; every reader of the mark
  takes its kind as the part after `contest:` and before any further `:`
  (`contest_name`), so the board's kinds, History and the backups' merge
  read it as its contest, and `SessionResult::assisted` reads the mark, or
  the log where the mark is plain.

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
message (`state.rs:319-326`) says "Restored 12 sessions and filled in 3,
with your settings from 19 Oct", or "Filled in 3 sessions and restored your
settings" when nothing was added.

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
(`HistorySaved::Squeezed`) gains "Long contest runs take more room" when
the history held a run with its log (`Squeezed { kept, runs }`), and
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
| Calendar, streak, accuracy | Count; character accuracy covers calls and fields, lost copies included. A day's best reads "Best points" (`heatmap.rs:231`). |
| Letters (`stats.rs`, `character_diagnostics`) | Accuracy counts every keyed character. Recognition times count a run's calls and exchange tokens by the rule every mode has (docs/recognition.md): each character from its sound going to the output to its key in the field, for a call or token heard once, whole and alone in a turn that played on time, typed as sent, with Show copy off. Nothing stands in for an untimed one. |
| Mistakes, sampler beliefs | Count, on keyed tokens: K5XYC for K5XYZ is Z→C, `1T5` for `1N5` is N→T. |
| Sampling snapshot (`stats.rs:317-397`) | A Contest arm draws as the contest's `CallSource` does, the sampler's wanted call with the contest's `CallSpec` (`generate_contest_call`), for calls only, and says "the next caller's call"; the speed card says how callers' speeds come. As built with adaptive callers' speed, its chances are the next caller's as the run would draw it (`speed_forecast`, by `adaptive_speed_path`, which reads the callers' switch in Contest): leaning on your copy speed while they adapt, and even across the range while not. Adapting, its line reads "Chance of each speed for the first callers of your next run, and the share of characters you copy right at it. Within the run they come faster as you copy calls and numbers clean, slower as you miss them. The run screen's − and + move only your own speed.", as the chances are the belief the run starts from (`SpeedBelief::from_history`), and it adds "Your copy speed is above this range, so most come at its top: a wider range on the Speed card lets them go faster." when more than half the chance sits on the range's top speed, or the same of its bottom ("…below this range, so most come at its bottom: … lets them go slower."). With the switch on and one speed set it reads "One speed is set, so every caller comes at it. The run screen's − and + move only your own speed."; off, "Callers' speeds are drawn evenly from your range. The run screen's − and + move only your own speed."; either then "Right is the share of characters you copy at that speed." |
| Copy span, group-length forecast | Not contest measures, as for Callsigns (`ui/stats.rs:211-245`). |
| Copy speed (`speed.rs:285-310`) | A Contest pill and series, from call groups and numeric fields (NR, ZN) only, found by the log's layout (`ContestRun::group_copies`); a stripped run, or one whose log this build cannot read, uses the groups `parse_callsign` accepts. `adaptive_speed_path` was `None` in Contest until adaptive caller speed came, and the card read "Callers' speeds are drawn evenly from your range; − and + move only your own" (as built after operator feedback; it said − / + shifted them during a run), its chip "Not adaptive". As built with adaptive callers' speed (2026-10-22), the series is what each run's callers start from (§6.6), each call and number counted whole, a character lost off its end a miss (`whole_evidence`), and `adaptive_speed_path` reports them: in Contest it follows the callers' switch, `contestAdaptSpeed`, not group training's (`adaptive_speed_on`). While they adapt the card's chip reads "Adapting · Balanced", as the aim steers them, and its line "Each new caller's speed is drawn from your copy speed, within your range: faster as you copy calls and numbers clean, slower as you miss them. The run screen's − and + move only your own speed."; with the switch on and one speed set the chip reads "One speed set", as nothing adapts but the switch is not off (group training's card reads so too with its switch on and one speed), and the line "One speed is set, so every caller comes at it. The run screen's − and + move only your own speed."; off, the chip reads "Adaptive callers off" and the line "Callers' speeds are drawn evenly from your range. The run screen's − and + move only your own speed." A run whose callers adapted is a run like any other in the series, its copies what it copied. |
| Board (`board.rs`) | A Contest pill with a chip per kind (`BoardOf::Contest`, `board_kinds`; the first kind is chosen until another is), and the pills show when there is more than one mode or kind; `board_place` keys on the mark's kind. A run whose callers adapted to your copy is placed as any run is: points per group already weigh the speed each copy was sent at (§10, below), so faster callers copied clean earn more, and a run is not marked for how its callers' speeds were drawn. Ranked by points per group within a kind; a run qualifies when its log reads and it ran 5 minutes or more, to a limit of 5 minutes or more, with a clean QSO and the 10 groups every session needs (a WPX QSO owns two); a run whose log an older build dropped is unplaced. A board of runs alone, whichever pill is chosen, names that rule in its note, and an empty one whose scored sessions are all runs says it with Minutes per run in place of a longer session. Under All, WPX runs sit about 30% under Callsigns per group, since fields are short; the note says so when the board mixes runs with group sessions. The kind chips are tinted when chosen, not filled as the mode pills are. Assisted runs are greyed, unplaced, and out of record and form: as built (4b), a whole run with Show copy on is listed after the places on every board that holds it, its place a dashed `—` read as "Not placed", its figures greyed and "· assisted" after them, with a line saying why (`assisted_runs`); it marks nothing on the timeline. As built (4b), the Contest pill shows once the history holds a run, placed or not, with a chip for every contest it holds runs of (`board_modes`, `run_kinds`; the first with a run on the board is chosen until another is), as the Runs card is under it: a board of runs alone offers All modes and Contest. |
| History (`ui/stats_detail.rs:362-371`) | Every row names its mode (phase 0); a contest row opens its results; a stripped run says "log not kept by an older version". As built (4b), an assisted run says "assisted" after its contest, by its log or its mark, and its results say "Assisted: Show copy was on. The run counts in every stat, and is never placed on the board." |
| Auto level | Untouched: the contest finish never calls `complete_session` (`auto_level.rs:285-308`). |
| Runs card (phase 4) | Every run per kind: date, length, QSOs, rate, both scores, error share, best verified per kind and length. As built (4b), on Stats → Scores under the Contest pill, for the contest its chip chooses, below the board (`src/ui/scores/runs.rs`, `contest_runs`): newest first, each row dated as the board dates it, day and time ("Mon 5 Oct 2026 · 14:13"), so runs of one day tell apart, then "10 min" or how far a run ended early went ("03:12 of 10 min"), clean and logged QSOs, the rate (a Simple QSO's minutes a QSO), the share of QSOs logged with an error and its Points, and at its end the verified contest score over the raw one (a Simple QSO's Points; a run of a scored contest whose log does not read, "—" over its Points, so the column holds scores alone). The best verified run of each length (the limit it was set to), Show copy off and above zero, is marked Best, the first to reach it holding it; a contest without a score marks none. An assisted run is marked "assisted" and is never Best. A run whose log this build cannot read, or an older one dropped, is listed with History's words and opens nothing; any other row opens the run's results in the tab's place, with Scores at its top and Back at its foot, as a History row opens them in its list's place. History's words for a log, and its QSOs and rate, are History's own (`log_parts`). The card lists the newest 40 runs, and the best of each length wherever it falls, with "Show n more" for the rest, as History pages its sessions. Each part of a row is kept whole, so at 360 px a row takes three lines and nothing scrolls sideways. |

**Two scores, two jobs.**

- **Dust points** measure copying: 10 × characters × speed × errors × time ×
  repeats, weighted by the alphabet's entropy (`score.rs:116-142`). From
  phase 1 they read "Points" where today they read "Score" (`results.rs:133`,
  `heatmap.rs:231`) or a bare number (`stats_detail.rs:370`, now "496 pts").
- **The contest score** measures operating. `contest_score()` works it out
  from the log: Raw, every logged non-dupe QSO's points × the multipliers
  as typed; Verified, clean QSOs only, as MRCE's (`Log.pas:650-680`); rates
  and the best 5 minutes. Its columns read "QSO pts · Mults · Score". As
  built (2b), raw's multipliers are those of the QSOs it counts and of
  those verified counts: where a field is the multiplier, as the Sprint's
  state, a clean QSO after a NIL with its call is verified with the state
  it sent, though raw drops it as a call typed again, so raw counts that
  state too, and verified is never above raw. A dupe's state counts in
  neither, as a logger counts none.

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
  shared `callsignLevel`. As built (3b), a mix of countries (Field Day's)
  is settled once per caller, before the candidates (`for_one_call` in
  `wanted_call`), so the lean picks among calls of one side and leaves the
  mix at its counted share.
- **The generator.** `generate_call(tier, spec, rng) -> GeneratedCall
  { text, country, digit, host, marker }` in `callsign.rs`. A `CallSpec`
  filters countries (`Any`, `WVe` = K and VE, `NotWVe`, `NorthAmerica` = W/VE
  plus Alaska, Hawaii, Puerto Rico and the US Virgin Islands, the stations
  an ARRL or RAC section covers, and, as built (2b), `UsAndCanada` = W/VE
  plus Alaska and Hawaii, who call in the Sprint, each with a state or
  province to send). As built (3b), Field Day draws from `FieldDay`:
  `NorthAmerica`, and 0.9% of its callers from `OutsideNorthAmerica`, every
  other country, who send DX (§3, §11.5).
  A station abroad is in its host's country. A contest spec
  (`CallSpec::contest`) uses contest shares for portables (4%) and abroad
  calls (3%) in place of the tiers' 25% and 12% (`callsign.rs:44-47`), and
  never draws Guantanamo Bay's KG4 with two letters (§11.2). With the
  default spec, which Callsigns mode and the band's traffic use, it draws
  exactly as `generate_callsign` always has, KG4 with two letters among its
  calls, which a golden list pins, with a case at a seed where it draws one.
  As built, after a review of group training: slice 3a's filter applied to
  every spec, so Callsigns mode never sent those calls (a few in 20,000 at
  tiers 3–6) and a seeded stream shifted after the first; it is the contest
  spec's alone now.
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
  As built (3b), the FCC's download (the ULS amateur licence file) was
  refused from the build's network, so the 2020 Census shares stay, and
  `places.rs` says so in its header; a script counting the ULS file can
  replace them.
- **Canada by digit, for every prefix** (VA, VB, VC, VE, VG, CF–CK, XJ–XO;
  the table holds VE, VA, VC, CK and VB, `prefixes.rs:203-209`): 1 NS, 2 QC, 3 ON, 4 MB, 5 SK, 6 AB, 7 BC, 8 NT,
  9 NB. VO1 NL; VO2 LB in ARRL DX, NL elsewhere; VY0 NU, VY1 YT, VY2 PE.
- **As built (2b),** `contest/data/places.rs` holds the codes and names,
  and draws a caller's place from its call once, when its exchange is
  first fixed, so a dupe sends the same: the call area is a call-area
  digit after the slash where there is one (`W1AW/4` is in the fourth
  district), or a host's (`W1AW/VE3` is in Ontario), and one that names
  no place (`VE3ABC/0`) leaves the station where its own call is. A
  call-area digit after a Canadian call's slash is a province by VE's
  digits whatever the prefix (`VY1ABC/2` is in Quebec); VO's and VY's own
  tables read only their own digits. Within a district the shares are the
  states' people at the 2020 Census, in thousands. VO2 is NL, as LB comes
  with ARRL DX. A special-event prefix's digit means what it means under
  the prefix it stands in for, which can change from one event to the
  next: CH and CI stood in for VO and VY in 2017, so Dust reads them as
  Canada's and no place from their digits, and the digit rule above holds
  for the rest of the prefixes listed. Where a station is at home, the US
  or Canada, Alaska and Hawaii among them, or elsewhere, is read there too
  (`in_us_or_canada`), from every prefix the US gives Alaska (AL, KL, NL,
  WL) and Hawaii (AH, KH, NH, WH with a 6 or a 7; the other digits are
  Pacific islands no state holds) and every prefix Canada issues, as the
  call table holds only some of them; else from the table.
- **W/VE side.** Your country, after any host, is K or VE. KH, KL, KP, AH,
  AL, NH, NL, NP, WH, WL and WP prefixes are DX, as are CY0 and CY9, and
  KG4 with a two-letter suffix (Guantanamo Bay).
- **As built (3a),** the side is read in `places.rs` (`w_ve`), from where
  a call is at home as the places read it: the US's call districts and
  every prefix Canada issues are W/VE; Alaska's and Hawaii's prefixes, the
  islands' (which the table places in Puerto Rico or the Virgin Islands),
  CY0 and CY9 (which it does not place), Guantanamo Bay's KG4 with two
  letters, and every other call are DX. A test pins each prefix above with
  every digit. LB comes with ARRL DX: a VO2 caller's place is LB there
  (`draw_place`'s `labrador`), and NL in the Sprint, as before.
- **The country a contest scores** a station in (`home_country`) is read
  from the same: K for the US's call districts, KL for every prefix Alaska
  has, KH6 for Hawaii's with a 6 or a 7, VE for every prefix Canada issues;
  KP4 for KP, NP and WP with a 3 or a 4 and KP2 with a 2, where the table
  reads KP2 as Puerto Rico; none for the Pacific islands KH signs
  with another digit, Navassa (KP1), Desecheo (KP5) and Guantanamo Bay,
  which the table does not hold (it reads KH2 as Hawaii and KG4AB as the
  US); and else the table's. CQ WW's points and multipliers, your own
  country and continent kept with a run, and the line under your call all
  use it, so they agree with the side and the zone. The generator never
  draws KG4 with two letters for a contest's callers (`CallSpec::contest`),
  as the table counts KG's two-letter suffixes from its other digits and
  would label such a call the US's; a test pins that every call a contest
  draws is in the country the generator labels it with. Practice draws
  them as it always has (§11.1): in Callsigns mode and the band's traffic a
  call is scored in no country, and the calls Callsigns mode sends never
  change under it.

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
(VER20200405, which `prefixes.rs` names in its header). A test pins it row
by row.

As built (3a), `scripts/callsign-prefixes.py` writes, after the call
shapes (which stay as they were), each country's name, CQ zone and
continent as cty.dat gives them (`COUNTRIES`), and the CQ overrides that
fall on a prefix and its digit (`ZONE_PREFIXES`): China's 52 (`BY0`,
`3H9`). Three groups of overrides are left out. Those that turn on the
suffix's first letter, Russia's oblasts (`R8T(16)`), as the table reads a
call by its prefix and digit; so Asiatic Russia is zone 17 throughout, and
`R8(17)` is its country's zone already. Canada's (`CH2(2)`, `VO2(2)`,
`VY0(2)`, `VY1(1)` and the rest), as Canada's zones are entered by hand.
And Antarctica's (`AX0(39)`, `VI0(39)`, `VK0(39)`, `ZL5(30)`, `ZM5(30)`,
`ZS7(38)`), as Antarctica is not a country of the table. An entity that
counts only for the WAE award keeps its prefixes' zone and continent where
they differ from the DXCC entity its calls are counted under
(`CONTINENT_PREFIXES`, and `ZONE_PREFIXES`): European Turkey's TA1, TB1,
TC1 and YM1 are in Europe, though Turkey is in Asia, and African Italy's
IG9 and IH9 in Africa, zone 33; so a TA1 caller is a point from Germany,
not three. The script writes the cty.dat version into the header. The US
and Canada are left out of the zone overrides:
`contest/data/zones.rs` holds every state, DC, province and
territory, with Labrador, and the two split places with the share of
their stations in the other zone (QC 2 at 1%, NU 1 at 10%); VK6 and VK8 are
29 there. A caller's zone is drawn: its state among its district's by
their people, then a split place's zone by its shares. Your zone from your
call is the one most stations there are in: a district's zone with the
most people (K7 is 3, K4 is 5, K8 is 4), or your state's where you give
one, even in a contest that does not send it; `myZone` overrides both. A
call in the US or Canada whose digit names no place (CH1, VE0) reads as no
zone until you give your state or province, which then gives it; a call
Dust cannot place (C31, KH2) reads as none, and `myZone` gives it.
Labrador is zone 2 whatever the contest. A caller's zone is never missing:
every call the generator draws in the US and Canada has a place.

### 11.4 Sections

The 85 ARRL/RAC sections are the non-DX codes in MRCE's `FDGOTA.txt` (less
one stray `MD`), as of
RAC's 1 January 2023 changes (MAR became NB and NS, NT became TER, GTA
became GH). RAC's YT (1 July 2025) is not in ARRL contests: callers send
TER; the parser and `mySection` accept YT. Sources disagree on Yukon's code
(YT or YUK), so the choice is to be checked against ARRL's list (§14). The
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

As built (3b), `contest/data/sections.rs` holds the list by call area,
each section named (as its place is, where the code is a place's, else in
the file: "Eastern Massachusetts"), and reads a section's code
(`listed_section`); YT reads as itself and an ARRL contest counts it as TER
(`fold_yukon`), and nothing else is read, NFL being Northern Florida and
the old codes (MAR, NT, GTA, NF) none. Yukon's code: what the build could
reach disagrees. A map of the sections kept up with RAC's announcements
gives YT, and says the YUK of the first report (*The Canadian Amateur*,
July/August 2025) was a mistake; news reports of the change give YUK;
ARRL's and RAC's own lists could not be fetched, and searches after
review found both codes again, in summaries that could not be opened and
checked. Dust reads YT, as this section has it, and the file's header
records the doubt, so the choice is to be checked against ARRL's
contest-sections list, and RAC's, once they can be reached. The 2026
Field Day entries have Yukon stations sending TER, as callers here do.
Where a call tells its section by itself is read in `places.rs`
(`call_section`): every prefix the US gives Alaska is AK; Hawaii's with a
6 or a 7, and with a 0, 2 or 8 (the Marianas, Guam, American Samoa), PAC;
the Caribbean islands' with a 3 or a 4 PR, and with a 2 VI; CY0 and CY9
NS; and every other call outside the US's districts and Canada DX, KH's
other digits, Navassa's KP1,
Desecheo's KP5 and Guantanamo Bay's KG4 with two letters among it, as no
section lists them. A call in the districts or Canada, or one Dust cannot
place, tells none. A caller (`contest/data/fd.rs`) sends the section its
call tells, or else one of the sections of the place `draw_place` draws
for it, as often as that place's entries are in each; so AK, PAC, PR and
VI callers come as often as the call table draws KL, KH6, KP4 and KP2
calls, which are counted from real calls. A test pins every prefix and
digit above.

### 11.5 Exchange values

- **Names.** About 150 short North American given names from the SSA
  baby-name data (public domain), weighted to births 1935–1975 and folded
  to the forms operators send (ROBERT → BOB); short lists per region written
  for Dust elsewhere. A name is kept per caller, never tied to a call. The
  16 names at `band/activity.rs:1152-1155` move here. As built
  (`contest/data/names.rs`), the network refused the SSA download, so the
  North American list is written by hand from common short given names of
  those years, each weighted by Dust's estimate of the births that go by
  it, and its header says so; a script counting the SSA's files can replace
  it. About twenty regional lists, drawn evenly, are chosen by the caller's
  country, and any country they do not list, the English-speaking ones
  among them, takes a British and Commonwealth list. No name reads as a
  report (`ANN` would be 199). The band's ragchews draw from the same lists
  by their station's country.
- **Field Day class,** from 13,709 rows of `FDGOTA.txt`: 1D 41.9%, 1E 14.0%,
  1B 13.1%, 2A 6.7%, 3A 6.7%, 4A 3.5%, 1A 2.9%, 5A 1.8%; 1C, 2D, 2F, 2B, 2E
  about 1% each; the rest 4%. As built (3b), `scripts/contest-data.py`
  counts them, from the 13,708 rows left once the one row whose section is
  no section (`MD`) is dropped, into `contest/data/counted.rs`: the shares
  above, then each class that rounds to at least 0.1% (3F, 6A, 1F, 4F, 7A,
  3E, 3D, 8A, 5F, 9A, 2C, 4D, 6F, 4E). DX rows are 0.9% of them, the share
  of Field Day callers that are DX, through the sampler too (§11.1). Each
  place's sections are counted from the same rows and rounded by largest
  remainder, so each place's add up to exactly 100%.
- **Power,** from 3,779 DX rows of `ARRLDXCW_USDX.txt`: 100 34.7%, KW 25.0%,
  500 12.8%, K 10.9%, 400 4.6%, 200 3.0%, 5 2.4%, others 6.6%. As built
  (`contest/data/power.rs`), the others are the next counted values at
  their shares (300 and 50 at 1.2%, 700 at 1.0%, then 99, 600, 750, 150,
  1K, 800, 80 and a tail at 0.1% each), each sent as written. As built
  (3b), `scripts/contest-data.py` counts them too, leaving out what is no
  power Dust reads (`5W`); it reproduces the table slice 3a wrote by hand
  exactly, value for value and in the same order, which now lives in
  `counted.rs` beside the Field Day shares.
- **Reports** (Simple QSO), Dust's estimate: 599 40%, 579 20%, 589 15%,
  569 10%, 559 8%, 449 4%, 339 3%.

### 11.6 Serials, cut numbers, 5NN

- **Callers' serials** are `1 + floor(final × (T0 + t) / L)`, never past
  `final`: `L` is the contest's length (48 h, 4 h), `T0` uniform in
  [0, L − the run's limit] (60 minutes for an open run), `t` run time,
  `final` capped at 9,999 so a serial stays within four digits. `final` is
  log-normal: WPX median 190, σ 1.45, fitted to F6FVY's histogram of last
  serials in MRCE (`CqWpx.pas:127-196`); Sprint median 80, σ 0.9, Dust's
  estimate (`SPRINT_SERIALS`, `L` 4 h). Those bins count logs, and a
  station you hear tends to have a big log, so `final` is drawn with
  weight ∝ final^0.5, Dust's estimate between logs and QSOs. A dupe sends
  its previous serial + 1 + Poisson(final × Δt / L). As built (2a), the
  dupe's serial is fixed as
  it calls again, Δt the run time since its last exchange was fixed, the
  count drawn as Knuth draws one, within four digits; the rest of its
  exchange is what it sent before, its name among it.
- **Habits `easy`:** plain digits, no padding (`7`). **`real`,** as MRCE:
  padded to three with the leading zeros as `TT` (`Station.pas:493-494,
  536-537`), 98% of other zeros as T, 9 as N 97% of the time (`:538-539,
  564-565`), zones and power cut to T, A, N for 70% of callers
  (`:555-562`), and `ENN` in 5% of messages (`:527-528`). As built (4a),
  checked against `Station.pas`: a serial is padded and cut only where
  the contest cuts yours, as WPX does (`serial_cut`), as MRCE pads and
  cuts only WPX's; a Sprint caller's goes in plain digits under every
  habit, as yours does, since a serial of nines and zeros cut would be
  letters alone (`NT` for 90), read as a state beside the name. Padded,
  every run of zeros is `T`s
  (`TT7`, `1TT`), as `000` and `00` always are, whatever else is cut; a
  zero alone is `T` 98% of the time, else `O` 40% of the time and `T` 97%
  of the rest, so `T` 99.2%, `O` 0.8% and a plain 0 hardly ever; nines are
  `N` 97% of the time. A power's zero alone is `O` 40% of the time and
  `T` 97% of the rest, with no first draw, and 70% of callers cut its
  ones and nines too. 599 is `5NN`, or `ENN` in one message in twenty:
  where the report is prefilled, drawn for each message, as MRCE draws
  it; where it is copied (Simple QSO), once, with the exchange. The cut
  forms of the copied fields are drawn once, as a caller's exchange is
  first fixed, so a QSO keeps one token for what was sent. `rough` sends
  as `real`, and adds the careless operators of §4.3.
- **Your numbers** are always cut, except your Sprint serial and Simple
  QSO's `599`, as its template writes it.
- **As built (3a),** a caller's zone and power go through `send_zone` and
  `send_power` beside `send_serial`, with the habits the caller is given
  (from 4a, the setting's): easy sends plain digits and power as
  written; real pads a zone below 9 with a zero 5% of the time, as MRCE's
  CQ WW does (`CqWW.pas:305-307`), and cuts its zeros, ones and nines for
  70% of callers; a power's run of zeros is always `TT` (`1TT`), a zero
  alone `O` or `T` as above, and 70% of callers cut its ones and nines too
  (`ATT`). A kilowatt goes as written.
  The three cuts share one helper (`cut` in `exchange.rs`).

| Field | Canonical | Equal when |
|---|---|---|
| NR, ZN | integer, no leading zeros | numbers equal (`TT1` = 1, `05` = 5) |
| RST | three digits | equal (`5NN` = `ENN` = 599) |
| ST | code | equal after PQ → QC, NF → NL; LB → NL in Sprint only (as built: in every contest but ARRL DX, `ContestRules::canonical`) |
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
  the last report-shaped token, NAME the last all-letter token that is not
  a report (`ENN` is a report). From Sprint,
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

As built (2b), `parse_exchange` reads every contest in the three passes,
WPX and Simple QSO as before. The rules above leave some tokens two
fields', so: a token a copied closed list holds is never a number (`TN`
is Tennessee, not 9, and a state put right, `145 BOB NE OR`, leaves `NE`
unused); where a name is copied, a serial or a zone has a digit in it, or
else is cut letters alone only while another token is left for the name
(`ANNE ON` is a name, not 1995, and `TNN BOB OR` is 99); a name prefers a
token no list holds, so `145 BOB TN OR` keeps BOB, and takes one only
when nothing else is left, and then the first of them (`145 AL AL`, and
`145 AL OR TN`, a state put right, keeps AL); a serial passes over a
report only in an exchange that holds one, so in the Sprint `599` is a
number like any other; and a name passes over a report-shaped token only
where a report is copied. As only the Sprint copied ST then, `Field::St`
read LB as NL outright. As built (3a), LB is a code of its own in
`Field::St`, and a contest reads values through its rules
(`ContestRules::canonical`, `same`, and the check and the groups' keyed
tokens with them): the row's `labrador` flag, set only for ARRL DX, keeps
LB apart, and every other contest reads it as NL, so the Sprint still
reads `7 MIKE LB` as NL and logs it right for a station that sent NL.
Power is a closed list that never takes what reads as the report
(`5NN KW` is a kilowatt, `5NN` alone no power), as no station runs 599 W.
As built (3b), Field Day's class and section are both closed lists: every
class has a digit and no section has one, so neither takes the other's
token, and with no number copied a section that would read as cut digits
(`ONE`, `TN`) is a section; YT reads as TER, and `3A WA` is Invalid, WA
being no section (Washington's are EWA and WWA).
The hint names every field
still to copy; where that and what Enter then does would not fit a
phone's line, it leaves the second to a wider screen ("Copy their
number, name and state").

### 11.8 Sources and licences

The repository is GPL-3.0-or-later (`Cargo.toml:9`).

| Data | Source | Status |
|---|---|---|
| Call shapes | `callsign/prefixes.rs` | counts only |
| DX country names, zones, continents | cty.dat (AD1C) | no licence found; facts only; ask AD1C; fallback CQ's country list |
| NA zones, places, sections | CQ, USPS, FCC, ISO 3166-2:CA, ARRL/RAC | public facts |
| State by call area | FCC ULS | US Government work, counts only |
| FD class, power, section shares | MRCE's `FDGOTA.txt` (W1MI, SM7IUN, W7SST), `ARRLDXCW_USDX.txt` (VE2FK, SM7IUN, K3CT, WV4P) | in an MPL-2.0 repository, compiled from contest logs; rounded shares only; ask W7SST |
| WPX serial fit | F6FVY's data in MRCE `CqWpx.pas` | two derived numbers |
| Names | SSA baby names; written by hand for Dust until they can be fetched (§11.5) | public domain; GPL-3.0-or-later |
| DX names, reports, Sprint fit | written for Dust | GPL-3.0-or-later |
| Caller behaviour | modelled on MRCE (MPL-2.0) | written for Dust; no code taken |

The counted shares are written by `scripts/contest-data.py`, whose inputs
are used where the table is made and never committed, so no test can run
it: `scripts/contest-data.py --check FDGOTA.txt ARRLDXCW_USDX.txt` says
whether the committed `contest/data/counted.rs` is what the files give,
and fails if it is not (as built, 3b, it is).

Dust is its own work: its sampling, noise model, signal processing,
envelopes and training parameters were built from scratch. Contest mode
took some ideas from how MorseRunner plays a contest, written for Dust and
taking none of its code, and the Credits card (`src/ui/credits.rs`) says so
in one line. Data sources are named in the tables' own headers, not on the
card.

## 12. Code structure

**Interfaces.**

```rust
pub trait CallSource {
    fn next_call(&mut self, s: &TrainingSettings, spec: CallSpec,
                 avoid: &dyn Fn(&str) -> bool) -> GeneratedCall;
    fn learn(&mut self, answered: Answered<'_>);        // session.rs:97-102
    // As built, adaptive callers' speed (2026-10-22): calls and numbers
    // teach the copy speed, and a newcomer's speed may be the source's.
    fn learn_speed(&mut self, _answered: Answered<'_>) {}
    fn next_speed(&mut self, _s: &TrainingSettings) -> Option<(f64, f64)> { None }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Turn { pub id: u64, pub at_run_ms: u64,
                  pub starts_at: Option<MonoMs> /* a continuation's splice (5a, 5.2) */,
                  pub segments: Vec<Segment> }
#[derive(Clone, Debug, PartialEq)]
pub struct Segment { pub who: Sender /* You | Caller(u32) */, pub text: String,
                     pub voice: StationVoice, pub start_sec: f64, pub end_sec: f64,
                     pub fade: Option<Fade> /* from phase 2, flutter in it from 5a (5.5) */ }
pub struct Fields { pub call: String, pub exch: String,
                    pub call_edited: MonoMs, pub exch_complete: Option<MonoMs> }
pub struct ContestView {
    pub phase: ContestPhase, pub clock: Option<RunClock> /* None in the countdown */,
    pub limit_ms: u64, pub my_wpm: f64,
    pub qso: QsoState,         // last call sent, exchange taken, corrected, F3's call, nobody calling, pile-up, a gone station's calls, the calls heard of stations gone, the call sent fitting only one
    pub air: Option<AirView>,  // heard from, your text and where each character ends, segment spans
    pub shown: Vec<ShownText>, // with Show copy on (4b): each station's text and when it shows
    pub log: Vec<LogRow>,      // oldest first: n, time, call, fields, wpm, asked, Pending | Checked
    pub qsos: u32, pub clean: u32,
    pub serial: u32,           // your next serial; 0 without one
}
impl ContestView { pub fn run_ms(&self, now: MonoMs) -> u64; pub fn rate(&self, now: MonoMs) -> Option<f64>; }
impl AirView { pub fn still_sending(&self, now: MonoMs) -> &str; pub fn calling(&self, now: MonoMs) -> usize; }
```

Nothing in the view moves with the time alone, so `view()` takes no time:
the run bar reads the clock and the rate off it as it ticks, and the air
line what is still to go of your message.

Effects carry `Turn`, which derives `PartialEq` (as `StationVoice` does,
`station.rs:12`), so machine tests compare effects as the group tests do
(`machine.rs:622-628`). The runtime calls `plan_turn(&turn, &settings)`,
then `lease.send_planned(asked, &plan, &transcript, &settings, turn.starts_at,
on_start)` with `on_start: impl FnMut(f64)` taking lead plus latency in
seconds, a continuation's wait for its moment among it (as built, 5a), and
`asked` from `lease.ask_to_send()` as the turn goes on the air (5.2). The
view holds no caller truth until a row's check resolves, but for what Show
copy shows, each text with the moment it may show, and is published only
when it changes (`session_runtime.rs:297-303`).

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
`ctx.announced` once nothing is on the air, held until then; the one
polite region that speaks it sits in the app's frame, so what the run
says as it ends is still said on the results. One `use_fields`
registration routes paddle letters to the focused field and the paddles'
word space to `field_space` (5b); a Space, an edit or a field change made
by hand calls `ContestEntry::finish_letter` first, which also ends the
paddles' word. `app.session` holds `Live::{Groups, Contest}`, each boxed;
`finish_run` and `abort_run` reach whichever is live.

**New in cw-core, `crates/cw-core/src/contest/`:**

| File | Holds |
|---|---|
| `mod.rs` | `ContestKind`, `RunStyle`, `CallerHabits`, `TuCall` (forgiving serde; `TuCall` from the operator feedback), `CallSource` |
| `rules.rs`, `esm.rs`, `clock.rs` | the `ContestRules` table, `tu_adds_call` (operator feedback); `esm()`, `Fields::edit`, `space_to_call` (operator feedback); `RunClock` |
| `machine.rs` | `ContestMachine`, events, effects, `ContestView` |
| `caller.rs` | caller states, patience, matching, replies, collisions, QRS |
| `turn.rs` | `Turn`, segments, continuations, `plan_turn`, `MONITOR_LEVEL` (your sending's loudest, operator feedback), per-caller fades and flutter, the receiver muted while you send (`Turn::muted_until`, `Turn::muted`, 5a) |
| `place.rs` | `place_contest_caller`, `filter_capacity` (phase 2); `place_qrm` (4a) |
| `qrm.rs` | the QRM of rough habits: when a station starts up, what it sends and where in a turn (as built, 4a) |
| `exchange.rs`, `parse.rs` | canonical forms, cut numbers, keyed tokens; `parse_exchange` |
| `serial.rs`, `check.rs`, `score.rs` | serials; `QsoCheck`; `contest_score`, rates |
| `record.rs` | `ContestLog`, `ContestRun`, `QsoRecord`, `result()` |
| `bot.rs` | operators that hear the turns; Hear a QSO uses the perfect one (as built, 2c: shipped, `hear_qsos` and `HeardQsos`; the test operators, scripted calls and run settings kept to tests in `bot/script.rs`) |
| `data/` | `names`, `reports` (1c); `places` (2b, the ARRL DX side and Labrador 3a), `zones` and `power` (3a); `sections`, `fd`, and `counted`, the shares `scripts/contest-data.py` writes (3b) |

**Changed in cw-core:** `Cargo.toml` (`serde_json`); `settings/mod.rs`,
`contest.rs` (new), `contest_preset.rs` (new, 2c), `save.rs`, `reset.rs`,
`clamp.rs` (`Contest`, `ALL` of 6,
`draws_characters`, `sends_callsigns`, `ContestSettings`, `contestActive` on
disk, `Station`, `wild`); `settings/curriculum.rs` (Contest → `LevelAxis::Callsign`,
`:49-56`); `session.rs` (`StoredSession`, `mode_on_disk`, `recorded_mode`,
`contest`, a test constructor, `MonoMs::plus_sec`); `callsign.rs` (`generate_call`, `CallSpec`, public
`parse_callsign`, `wpx_prefix`, `country_of`); `sampling.rs`, `pool.rs`,
`auto_level.rs` (Contest arms; `impl CallSource for Sampler`, and, as built
with adaptive callers' speed, its callers' speeds from the copy-speed
belief, `adaptive_speed_on` reading the callers' switch in Contest);
`stats.rs`, `board.rs`, `speed.rs` (history rows, kinds, assisted runs,
contest speed evidence, `tells_speed`, `whole_evidence`);
`timing/pileup.rs` (reach helpers `pub(crate)`); `timing/plan.rs`
(`dry_wanted`, `plan_morse_playback_at`, 1b; `muted_until`, and
`SEND_RELEASE_SEC` moved here from the app, 5a); `band/qsb.rs`
(`Qsb::flutter`, 5a); `keyer/clock.rs` (the word gap the paddles' field
space keys, 5b); `soak.rs`, `lib.rs` (the contest soak; exports).

**New in the app:** `src/contest_runtime.rs` (`LiveContest`, start,
dispatch, effects, `play_turn`, finish, the heartbeat check);
`src/ui/contest.rs` (run bar, hint, air line, fields, key bar, log, paused
panel, the pause on a hidden page, whose visibility listener and minimise
handler are in `src/page_hidden.rs` as built); `src/ui/contest_keys.rs`; `src/ui/contest_results.rs`;
`src/ui/settings_station.rs`; `src/page_hidden.rs` (the page going out of
sight, for the run and for Hear a QSO, 2c); `src/ui/hear_qso.rs` (the dialogue of
Hear a QSO, 2c); `src/ui/scores/runs.rs` (the Runs card, as built 4b);
`scripts/contest-data.py` (counted shares).

**Changed in the app:** `src/session_runtime.rs` (`Live::{Groups, Contest}`,
`start_session` routing at `:180`, `finish_run`, `abort_run`);
`src/state.rs` (`contest: Signal<Option<ContestView>>`, `ContestEntry`,
`session_running`, the restore message); `src/routes.rs` (Training and
Results branches); `src/app.rs`, `src/ui/tour.rs` (Exit and Discard,
contest keys, tour button; the contest guide, 5b); `src/ui/paddle.rs` (a
word space keyed to a screen of fields, 5b); `src/keep_awake.rs`;
`src/player.rs`, `src/audio/mod.rs` (`send_planned`, `start_planned`,
`Lease::wake`, a recorder that keeps each start's plan and `Qsb`);
`src/audio/mixer/*`, `src/audio/web/*` (the dry path; the gate and a send
started at a given time, 5a), `src/audio/shape.rs` (the gate's ramp and
the browser's gate schedule, `GateSchedule`, 5a), `src/audio/silent.rs`,
`src/audio/wait.rs` (a send waiting for its moment is not stalling, 5a);
`src/ui/training.rs`, `widgets.rs` (`Keypad` rows as props,
`ProgressHeader { figure, pct }`); `settings_charset.rs`, `settings.rs`,
`home.rs`; `results.rs`, `heatmap.rs`, `stats_detail.rs`, `scores/mod.rs`
("Points", rows that open results, kinds); `listen.rs`, `playback.rs`;
`privacy.rs`, `credits.rs`, `docs/privacy.html`; `src/persist/backup.rs`;
`assets/styles.css`; `docs/platforms.md` (new modes write a legacy name
plus a mark); `scripts/callsign-prefixes.py` (zones and continents).

## 13. Testing

**Unit tests (cw-core).**

- **Parse:** every row of 11.7; fuzzing (no panic, idempotent on its own
  output); every equality in 11.6; no cut letter outside a numeric field.
- **ESM:** each row of 4.4 with its label, the states `C N V̄` and `C N̄ V̄`,
  Invalid sending nothing, focus moving to Exchange, `Fix call` after a
  correction.
- **Matching:** exact, close, none; `?` inside and at the end; best
  confidence only.
- **Callers:** patience over 10,000 draws in 4–14, mean near 7, four
  tries under 5% of the time and five or fewer under 25%; timeouts
  within ±50%; nothing sent after `Done` or `Gone`; CQ fails the worked QSO;
  a worked station leaves after 3 silent timeouts; each message answered
  while calling costs a try; a `?` after someone else's call is not for a
  caller, unless the Call field is empty; a partial while worked brings the
  call; an exchange after an answered partial is for the caller; a piece of
  the call asks, and a bare `?` is nobody's call; a partial it answers
  gives a caller full patience; a caller keyed over is never sent away; a
  caller back from waiting has fresh patience; a correction with your
  exchange in hand carries its own, five operators in six; from 2c, a
  caller slows down after three incomplete tries, or four of skill 3, by
  its skill's share and 2% more from the fourth, for slips, partials and
  a `?` while it waits for its call put right or your TU but not for your
  exchange; back on track it sends at its own speed, its call put right
  after your exchange keeping the slower one; a pile-up caller waiting
  out another QSO keeps its speed, a partial bringing it back slow and
  its CQ, TU, NIL, `?` with no call or own call at its own speed; and
  never below the Speed card's lowest speed. Hear a QSO's stand-ins leave
  nothing a run needs (`missing()` is none) for any station given.
- **Habits** (4a): each habit's number forms and shares, seeded (a
  serial's padding and runs of zeros always `T`s, a zero alone `T` 99.2%
  and `O` 0.8%, nines `N` 97%, 70% of zones and powers cut, a power's zero
  alone `O` 40%, `ENN` in 5% of messages, plain digits under easy); one
  caller in ten fumbling its first serial under rough habits and none
  under easier ones, the fumble a digit off by one and the serial right
  what was sent; one operator in about twenty sending its exchange early,
  after a slip or answering a partial or a piece while it calls, and a TU
  then completing its QSO; a slip taken for a call one time in 25, never
  a call of three characters; QRS under easy habits only; QRM stations
  starting up 15.5 an hour of run time, 46.5 messages, one to five each,
  never over itself, in turns as it comes due or as the next starts, and
  a long silence bringing one station, not a queue; a caller's exchange
  owed only the first time it takes yours in its QSO; the presets writing
  their habits; the callers' habits Band conditions', read back
  forgiving; Real's words on the Band card following the numbers each
  contest has you copy, and none in Field Day and the Sprint; a Sprint
  caller's serial plain under every habit.
- **Presets** (2c): each sets what it says and lands whole from any
  other; the rest of the settings, your station and the other levels
  among them, are left alone; a change to anything it set reads Custom,
  but a depth or level under a switch it turned off; with Farnsworth
  spacing set, it writes the spacing's range too; the defaults are on
  none.
- **Calls:** a golden list keeps `generate_callsign` unchanged, with a
  case at a seed where it draws KG4 with two letters, while no contest
  spec draws one; `parse_callsign` reads 10,000 generated calls per tier;
  each `CallSpec` gives only its countries; every WPX prefix case in 11.1.
- **Data and scoring:** the NA zone and province tables, LB, DC and the
  Sprint list pinned; 85 sections, named by list version; hand-worked logs
  per contest; verified ≤ raw, where a logged field is the multiplier
  too, and with a clean QSO after a NIL with its call, a dupe's state
  counting in neither; a dupe scores 0, is no error, counts clean; KL7
  and KH6 are CQ WW countries.
- **Field Day** (as built, 3b): the 85 sections, by call area, and RAC's
  2023 changes; YT equal to TER and no other alias; every place's section
  shares adding up to 100% and covering every section but PR and VI; the
  class shares; the section your call tells for every special prefix and
  digit, and `mySection` defaulting from it; `CL` and `SEC` equalities and
  `OR 3A` and its kin parsed; a hand-worked log scoring its QSO points;
  callers' DX share; the perfect operator working Field Day clean in both
  styles, from the US, Hawaii and abroad, and a QSO worked on the screen;
  the soak over all six contests. The counted table is checked against
  the script's output with `scripts/contest-data.py --check` (§11.8).
- **Turns:** `wanted` ends last; no reply is cut; your tones are unfaded,
  at your sending's level, none with it off and the turn as long; your
  message is dry and the callers are not; callers fade independently and
  smoothly; a continuation keeps every character of your message;
  capacity follows reach ÷ 60 Hz + 1; the band makes room for the callers
  sounding at once; from 5a, without full break-in the receiver is muted
  from the turn's start to your message's end, your sending off or not,
  and a caller's send any of which falls in it is lost to it, a reply after
  it is not, and nothing is muted with full break-in nor in a turn of
  callers alone; a flutter is laid in over the fading or alone, and never
  into your message; the word gap is a space's.

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
  a lost copy; a NIL adds a call group and a miss for each field; an empty
  run stores nothing; a NIL after the exchange was heard copies its fields
  as logged;
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
  goes only to the newest station heard, of single callers; a partial
  while the station waits is charged and timed;
- a TU without logging then a log is checked against that station, and Esc
  after it leaves that QSO to log; your exchange after an answered partial
  is answered; a question with the Call field empty brings the caller back;
  a piece of a call sent alone asks as a partial; a lost QSO keeps the call
  first sent with your exchange; Esc takes back nothing heard at an earlier
  key; a CQ that brings a call again costs its send; the station you work
  leaving is said, and Enter starts afresh; asking again for a call already
  typed costs it nothing; a bare `?` in the Call field is a question;
- Esc of the next call after a TU without logging keeps that QSO's copy,
  and that QSO is still lost if never logged; a correction drawn by a `?`
  reads Fix call; B4 or NIL sending the station away, or spending the last
  try of one that answered your partial, starts Enter afresh;
  a lost QSO owns a missed group for each field; a NIL before working the
  station costs its fields too; walking away once the exchanges are out is
  no way up the board;
- in pile-ups (2a): as many call as the range draws, as far as the filter
  fits; only the best match answers, and the station you work keeps a
  partial that fits it; a caller keyed over hears nothing, goes on through
  continuations and under a CQ, and calls again a moment after you; each
  caller's timeout is its own, and callers wait while you work someone
  else; a station you leave for another calls again; B4 and NIL send away
  only the station they are for, and the rest calls again; callers that
  only answered a partial lose no copy; a NIL like nobody's charges nobody;
  a station left for another, even with a slip of the next call, keeps
  the call you sent it, and Enter sends the next one your exchange; a call
  typed for another station at a key that sends no call is no copy of the
  one you work, nor a call kept for it once it has gone; a copy is crossed only by a
  station there when it is lost; a fix sent over a correction leaves Enter
  working and is not called for again; a caller that only answered a
  partial is charged no NIL and ends no QSO your TU closed; B4 with a slip
  of a call is put right; a timeout calls over other callers, whether they
  call alone or answer your CQ, and no turn of callers is cut short
  without a key; N is whether the station you work took your exchange, in
  a tie and with nobody worked; a near call, once the station took yours,
  is another caller's; a `?` charges only the QSO it was asked in; the
  hint says CQ once the station you work has gone; a typo that is no call
  goes to the nearest call; a caller keyed over on its last try stays in
  the QSO;
  dupes are one arrival in fifty once five QSOs are logged; your call
  brings newcomers; F4, F6, F8 and F12 send their messages;
- from the second pile-up audit: a caller on its last try answers your
  partial, and one keyed over on its last call calls again; callers who
  waited out another QSO come back to your CQ with fresh patience; a
  correction after your exchange carries the station's exchange, timed as
  one sent alone, and costs a send only when your `?` brings it again, a
  partial while you work the station charging it nothing; Enter reads the
  answer heard while the turn is still on, and one that has begun before
  its timer, and a correction is heard then too; a logged QSO's check
  shows while the pile-up calls on, and in a Simple QSO once the station
  has signed off; a turn never clears the band for more callers than call
  at once; a QSO logged for a station you left, a slip and all, is no
  copy of the one you work; a NIL while a dupe calls is charged to no
  older caller, and one while you work a dupe to no one but the caller it
  fits; a station goes on with one send when the sound is tried again;
- from 2c: a log after a TU without logging asks the caller its TU
  brought nothing, by Enter or `.`; a caller asked for its call again and
  again slows down from the third partial, or the fourth, its exchange
  back at its own speed once its call is heard right, its call credited
  at its slowest and its exchange at its own; a slow reply keyed over
  before it began credits nothing slower;
- from the third pile-up audit: B4 keyed over a dupe still calling sends
  its call again, and sends it away; Esc brings the pile-up back at once,
  as garbage does, and the station you work sends what it owes at no
  charge while a caller waiting out its QSO keeps quiet; a caller that
  missed your exchange still takes it late; once the station you work
  has gone, whatever call the fields hold for it (its call, a slip it put
  right, the call put right since, a partial it answered, its call typed
  since), the hint says so and Enter calls CQ, which the pile-up answers;
  after B4 sends a dupe away, B4 pressed again or not, or once a CQ brings
  a caller, Enter clears the call and charges no repeat, and F5 with the
  call keeps it gone; Esc keeps a call heard whole over your message, and
  puts back a dupe the message brought as it was; a stored run's log reads
  the speed its live log showed; a number logged before a correction's
  exchange reaches you is a miss, and a copy once it has. Unit tests: a
  station waiting for your exchange stays silent after its silent
  timeouts and takes it late; Enter calls CQ in place of a call whose
  station has gone, or clears it once someone calls; a correction's
  exchange starts after its two words.
- from the third browser test: in a CQ WW pile-up of five left to call,
  the call of one that has just given up, typed late, sends nothing over
  the callers still calling: the fields clear, the hint and the screen
  reader say it has gone, and the rest call on, none waiting on a QSO with
  nobody; its call stays gone, and once all have given up, Enter calls CQ
  in its place, which the pile-up answers. A call that fits a caller still
  calling goes to it, a slip and all, and a call nobody was heard to send
  goes out as typed; a partial and a slip of the call gone go out as typed
  while the rest call, and once nobody does, Enter calls CQ in their
  place, the hint and the screen reader saying the station has gone; a
  caller calling with the call gone again takes it; a dupe that gives up
  is gone too, while a station logged and off the frequency is sent its
  call as typed, B4 bringing the rest back; a single caller that gave up
  is answered with a CQ, and its call cleared once the next one calls; a
  caller that answered your partial and gives up on your `?` is said to
  have gone, and not again at Enter, as one worked that leaves on its
  timeouts. Unit tests: Enter never sends the call of a caller that gave
  up, whatever QSO it is in, a partial or a slip of it aside; Enter calls
  CQ for a partial sent to a station gone once nobody calls.

- from 4a: under rough habits QRM sends in a turn, from its start when
  it came due while all was quiet, through the receiver, never answering
  nor a station, out of RX's count and Enter's hint, the QSO going on as
  without it; QRM still sending goes on over your next message from where
  it was; a fumbled serial logged as put right is clean, its group the
  serial right, and logged as fumbled a wrong number; a slip a careless
  caller took logs busted with its own call; an exchange sent early,
  after a slip or a partial, reads as a correction, and the QSO logs
  clean with no repeat charged for the exchange it owed for yours; QRM
  carried over a key from a start retried six seconds late never sends
  over its rest; under real habits a cut serial copied in digits is
  clean.

- from 4b: with Show copy on, each caller's text shows a second after its
  send ends, kept once the turn is over and given way to by the next, none
  with it off; a call and an exchange whose text showed first keep what the
  fields held then, a later edit never changing that copy, the showing a
  send of its own, while the log keeps what was logged and is checked and
  scored the same from the stored run; a copy sent before its text showed
  keeps its own and no extra send; a pile-up caller keyed over is shown
  whole; a single caller's text showing while the Call field still holds
  the call of a station F3 thanked, or of one gone, keeps no copy of it;
  in a pile-up the calls shown while you type another keep nothing, a
  caller worked later keeping that empty copy, and a kept call another
  station then takes is no copy; a text not yet shown when a `?` goes out,
  or when the sound fails, is no cut-off, the next send's showing is; the
  run marked `contest:wpx:a` and stored assisted, the soak's every third
  run so, its stored score and log what the live run showed. Unit tests:
  Show copy's serde, its section and reset; the fields' text at a moment
  past, through any number of edits; assisted runs counted in the
  calendar, accuracy, Letters, Mistakes and copy speed; greyed and
  unplaced on the board, out of record and form; the Runs card's rows, its
  best of each length and its assisted and unread runs, and the Contest
  pill for a run too short for the board.

- from the operator feedback (2026-10-22): Space in the Exchange field
  goes back to Call, typing nothing, where the exchange has one item to
  type (WPX, CQ WW, ARRL DX from either side) whatever is typed, and
  where it has more (the Sprint, Field Day, Simple QSO) once what is typed
  reads whole, is nothing or ends in a space, a space typed otherwise; the
  keypad's `␣ Field` the same; the cursor at the end of the field it goes
  to, which the app's tests see in the focus script that moved it; with a
  correction to put right, the hint on a wider screen and the key bar say
  Space goes back only where it does, not in a Sprint's exchange typed
  partly, where the key reads Next item and a click types the space as the
  key does; Space never named like F2; your sending sets your tones'
  level, the native mixer's dry sample alone,
  and off leaves no tones and every time as it was, a QSO worked and
  logged clean; − / + moves your speed alone, from the whole speed nearest
  it, the callers keeping the Speed card's, the stored run's `my_wpm` what
  you sent, and the speed kept for the next run and stored once the run is
  over; − / + moving what the run bar shows by 2 from a middle on a half,
  shown as the Speed card shows it; your speed set sends at it, unset at
  the callers' middle, which a preset puts back; your call signs a
  pile-up's TU every QSO, every third or never as chosen, the callers
  calling after each, a single caller's never, and the card's example TU
  as your first QSO's would; the choice is the contest's where its TU
  leaves it, not the Sprint's or Simple QSO's; the three settings' serde,
  forgiving, their clamps and their sections; their controls, each on its
  card in Contest mode only (the TU's in a pile-up only), and their
  resets.

- from adaptive callers' speed (2026-10-22): a run copied clean draws
  faster callers than one whose numbers are missed, and by its end its
  source draws faster than a fresh one, the missed run's slower, every
  caller within the Speed card's range; callers already calling keep the
  speed they came with, and newcomers' differ; under easy habits a caller
  whose speed was drawn still slows down for you from it, back on track
  at it, its call credited at its slowest, the next caller within the
  range; a seed gives one run; with the switch off the run is the very run
  of a source that chooses no speed; the source learns the copy speed
  from calls and numbers, not from a report or a name, once the QSO is
  logged. Unit tests: the callers' switch is their own, group training's
  saying nothing of a contest nor theirs of a group session or of
  Listen's stream of groups, and one speed leaves nothing to adapt; calls
  and numbers copied clean draw faster callers and misses slower, a name
  nothing; a call or number copied without its end is a miss, live and
  from the history, an empty field nothing (`whole_evidence`); a run's
  callers start from the contest runs in the history, an assisted run as
  any, a Callsigns session not at all; the speed line keeps the spacing
  within the Effective range wherever the characters allow it, sending at
  the copy speed asked; the Sampling forecast follows the callers' switch;
  the switch's serde (forgiving, on), its section and reset, the reset
  putting back the Aim it brings onto the card, the presets leaving it
  alone; the Speed card's switch in Contest only, its words, the one-speed
  words naming only the ranges shown, the aim beside it while it adapts,
  and its reset; the copy speed and Sampling cards' words and chips,
  adapting, at one speed with the switch on, and off, the Sampling card
  saying when the copy speed is past the range.

- from 5a: without full break-in a pile-up caller sending across your
  message is not heard: its rest ends no copy's time, the copy timed from
  its call after you, RX counts it only from your message's end, the
  operators never hear it and Show copy never shows it, all of which full
  break-in hears; the station you work keyed over while its call put right
  goes out tells Enter and the hint nothing of the correction, begins no
  exchange, credits no speed and arms no heard timer for its rest, all of
  which full break-in hears; however late in your call the next key comes,
  the turn on the air sends every character begun before the continuation
  can take over and the continuation the rest, from where its first
  character starts, or half a word space after the call, the exchange a
  word space after it, or at once once the call has gone out, none twice
  and no gap; a turn started at once says the player's lead, a retry's or
  a continuation's start does not, and a splice waits out that lead and a
  release after the key; F5 then F2, Enter then Enter, and a `?` mid-CQ
  splice on from the next character. Unit tests: three callers in ten flutter under flutter,
  each its own rate and depth within range, the same seed alike, none with
  it off and every other draw of a caller the same; a flutter wanders at
  its own rate and depth; flutter and full break-in's serde (forgiving,
  off), their sections and resets; the presets writing flutter and leaving
  full break-in alone. The soak runs full break-in on half its seeds and
  flutter on two in five, each turn's muted span what full break-in says,
  every flutter within its range and only under flutter, your message never
  faded, and no continuation set to start sooner than a release after its
  key.

- from 5b: the keyer ends a word six dits of quiet after a letter's last
  element, a straight key's too, though its letter ends a dit sooner; a
  paddle closed before then carries the word on, and a key pressed in it
  ends the word with no space, as does a pattern that spells no letter;
  waiting out a word space is no letter keyed (`keyer/clock.rs`).

**Operators hear, never peek:** they read callers' text from `PlayTurn` in
cw-core and from the recorder's transcript in the app. Perfect: all
verified, at 3600·n / Σ(planned turns and drawn delays) an hour within 5%,
about 230 at 25 WPM for WPX. Human-paced (1.5 s per character typed, and
per key pressed): 60–150 an hour.
One digit off: verified 0. Unsure: asks `K5?` first, its call points
halve. Pauser: the clock equals the unpaused time. Dupe worker: 0 points,
no error. Hear a QSO (2c): three QSOs of each contest in each style, 24
seeds each, from the first CQ to the third TU and its sign-off, nobody
calling after it, a pile-up caller calling twice among them, the same
seed hearing the same QSOs; your station stands in until given. From 4a,
the perfect operator works callers of every habit clean in every contest
and style, reading cut numbers and `ENN` by value, a fumbled serial as
put right, and leaving QRM, which it hears but knows for no caller; Hear
a QSO stays clean under every habit, 24 seeds each, its last turn holding
no QRM, which it shows as a line of its own; and in the Sprint, 120 seeds
of each habit and style log three clean QSOs in under 40 turns.

**Soak** (`soak.rs`, seeds of its own: as built, 3,000 runs of single
callers and 4,500 of pile-ups, 14, shared among the contests that have
shipped, each held to a floor of QSOs logged of its own; from 4a, callers
of any habits, each habit held to a floor of QSOs logged, and in each
style numbers cut and `ENN` heard often enough under real and rough
habits, and fumbled serials, exchanges sent early and messages of QRM
under rough ones). Keys carry fields 40% as heard (a quarter of them with
the call heard before the last, as when logging a QSO left after F3), 20%
one edit off, 10% empty, 10% partial and 20% junk, among stale and
current turn events, timers by any id, pause, resume, hide and finish.

- Every step: no panic, fresh ids, a clock that never goes back, but for
  the retries a failed send gives back as paused (4.2); no station sends
  over itself in a turn, QRM among them, and pile-up callers sit 60 Hz
  apart or more; from 4a, QRM, `EEEEE`, `ENN` and exchanges sent early
  only under the habits that bring them, and callers' numbers in plain
  digits under easy ones. As built, a widened run on seeds of its own
  (9,000 single from seed 50,000 and 13,500 pile-up from seed 90,000)
  passed with QRM held to it too. As built with adaptive callers' speed,
  the callers adapt on every other seed, and every caller sends within
  the Speed card's ranges: its characters within Character speed, its
  spacing within Effective speed wherever its characters are as fast as
  its slowest, and never outrunning them; slower only as it slows down for
  you under easy habits, by at most 22% (`QRS_MOST`, skill 1's 20% and
  2% more), and never under the card's slowest speed.
- The end: serial = QSOs logged + 1; QSO times rise and stay within
  `run_ms` ≤ the limit; `run_ms + paused_ms` is the wall-clock span, or at
  most that when time ran out, as a run noticed past its limit ends at the
  limit (6.5); no reply ends after `wanted`; no row is `…`; `g` sums to the
  groups; every station worked, logged or lost owns its groups in exactly
  one QSO; in a pile-up no copy lost or charged as a NIL is a call
  another station there then took from you for its own (the call you last
  sent it, or first sent it with your exchange, or its own call once it
  heard one from you), unless this one took it too or the call is its
  own, looked at as each is lost or charged; points finite;
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
`wanted` last and no reply cut, and a dry `wanted` in every turn and in no
group session; a cancel never pauses, a failure does;
`on_start` dispatches `TurnStarted`; pause releases the audio and sends
`keep_awake(false)`; `Paused{Sound}` then Resume opens a player before the
1 s timer; a key typed between two renders survives; `Pause{Hidden}` and a
heartbeat gap both credit the gap as paused; a bad own call refuses to
start; a turn waiting out a retry never starts over a newer one, and an
Esc before a turn's task runs stops it. A unit test drives
`audio::silent::MorsePlayer` through a stretched turn and a cancel. Hear
a QSO (2c): on Listen it plays turn by turn through the run's path, dry and
fading off, each line shown once its send ends and not before, to the
third TU, each QSO after the first set apart, and nothing is stored; Stop
ends it and keeps the lines; another sound, a character picked or leaving
the screen puts them away; stopped before a line, the stage and the
card's QSO are as they were; the card plays it too, and another contest
or mode picked while it plays stops it and puts its lines away; it holds
the screen awake while it plays. What Start
keeps from Home's hero is stored as the run starts, and nothing more is
stored while it runs. Native
mixer tests (1b): your message a kilohertz off the narrowest filter still
peaks at the monitor's level; a weak reply right after it plays exactly as
it would alone, where through the receiver it was ducked; a stretched turn
finishes once its last reply is heard, dry or not; over a band whose
static and a full-level caller pull the AGC well down, your message adds
exactly its own tones to what is heard, unridden; and a dry send gives
your tones out unfiltered and unfaded, and stops them with the rest.
From 5a: without full break-in, once the gate has shut only your message
is heard, sample for sample, and once it has opened again after your
message the receiver is heard exactly as with full break-in, its AGC having
run on; the gate moves a straight line at a time, the same step a sample,
and never steps, a stop opening it; with your sending off nothing is heard
while you send; the paddle's sidetone is never muted; a fluttering caller
rises and falls with its own flutter, at 3 and at 33 Hz, half and three
quarters deep, two callers each with their own, the same turn the same
twice, and your message never fluttered; a continuation taking over where
the send playing gives way sounds every element where one send of the whole
message has it, to a sample or two, none twice and no gap; under a band and
a loud caller sending across, with your sending off, nothing at all is heard
and the gate never moves from the first send's gate shut to the end of the
continuation's message, a second key taking over from the first
continuation at its own start included, asked for a millisecond past it;
one taking over half a word space after your last character leaves that
element sample for sample as one send has it, the receiver back in that
word space and muted again from the takeover; the browser's gate schedule, a send on top going on from where
the gate stands without a step, a takeover of a takeover a millisecond
either side of its start, and a stop part way down a ramp; a send asked for
once the moment has passed starts at once, with nothing playing it waits
what it was asked, and stopped before its time neither it nor the send it
was to take over from is heard. The recorder keeps when a continuation was
to start: a key mid-CQ is spliced on where the Q starts and the run hears
it from then; every turn with a message of yours mutes the receiver to its
end without full break-in and none with it; a planned send asked to start
at a given time starts then, one asked for a moment past at once. The
settings screen's Flutter on the Band card and Full break-in on Tone and
volume, in Contest mode only, stored, flutter written by the presets and
full break-in left alone, and each card's reset.

**Render tests:** from 4b, a double tap on Pause leaves the run paused,
Pause's place inert and End run where it was; Resume, by its button, Alt+P
or after a hidden page, brings the run screen back to its top, and Pause
scrolls nothing; Show copy on the Session shape card in Contest only, its
reset and presets; the RX line showing what each caller sent, not before
its send ends and not read out, the results and History saying assisted;
the board's greyed assisted rows; the Runs card's rows, dated with their
time, marks, an unread run's dash, a page at a time with the best kept,
and opened results. Home in Contest with and without a call; no alphabet,
group, Experience or Auto level controls; Your station's fields and
reading; the run screen in countdown, running, corrected and paused; hint
and Enter label per ESM row; the keypad without `=`, `+`, `,` and `.`; the
longest Simple QSO and Sprint lines at 360 px; log rows with text; results,
"Points", History rows; the 3 + 3 grid; Listen in Contest with Hear a QSO
first, its lines, callers at once side by side, each QSO apart, which QSO
plays with Stop above the lines, no character picked meanwhile, its
stand-in, and none of another contest's or of one stopped before a line.
After the browser test of group training, a style test reads the rule for
phones under 760 px tall: the keys and every space of the training screen
it tightens, a run's and a session's, closer than a phone's, and a long
group answered kept to its slot, what it sent cut short and its
comparison's two rows on one set of columns that stop narrowing at a
letter's width; a session drawn on the app has the markup the rule names,
a `.train` root and the answered group's top row and comparison.
From 5b, in the desktop and the touch builds alike: in Contest the guide's
button shows, and its guide walks the run screen by hand, each part's id
drawn where the guide points (the keypad first where touch is how you
type, each build's words for it), docked clear of what it shows, then
Discard, asked twice, keeps nothing; Next from the first step to the last
starts no run and nothing sounds, the steps pointing the way to the run
and saying a run shows them; on a paused run the steps for what its card
stands in for point at Resume and say so, the rest as ever; Show me
starts the run and, on its step, discards it; picking Contest or leaving
it starts that mode's guide; on the run screen the guide's buttons, Start
practising among them, leave the cursor in the run's field and a press
hands it back; the script that brings a control into sight keeps it clear
of the dock, at its foot or its head. The paddles keyed into a run at the
keyer's speed: a word space moves the cursor by the Space key's rule, on
to Exchange, back to Call from a WPX exchange, a space between a Sprint's
items, the cursor at the field's end and nothing typed; a hesitation of
six dits keeps a call whole; a pause presses Space once; and none follows
Enter. A Space by hand in the word space, from the keyboard, the key bar
or `␣ Field`, moves the cursor once, and typing or a focus moved to the
other field leaves it where it is: the word space adds nothing after any
of them; and a letter still open when Space comes lands first, in Call.

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
- **1b Honest clock and sound.** The dry path for your message. The rate
  and − / + came with 1a's run screen. Landed: `plan_turn` marks your
  message `dry_wanted` at `MONITOR_LEVEL`, and both players play it around
  the receiver, through the limiter (5.3).
- **1c Simple QSO.** Names data, the RST and NAME exchange, `myName`.
  Landed.
- **1d Contester keys.** F2, F3, F5, `;`, `.`, `,`, `+`, Shift+Enter, and
  their buttons on the key bar. The key bar, browser defaults kept from
  the run's keys and Esc locked in fullscreen came with 1a's run screen.
  Landed.

**Phase 2: pile-ups and Sprint,** the first release.

- **2a Pile-ups.** `contestStyle`; arrivals from the stations range;
  `place_contest_callers`; per-caller QSB; best-match partials; collisions
  carried through continuations; NR?/AGN; calling twice; dupes, B4 and NIL;
  F4, F6, F8, F12; "2 calling"; the Band card labels. Landed, with the
  contest soak run in both styles, each on seeds of its own (2,000 of
  single callers, 2,500 of pile-ups, where callers drop out of a QSO you
  leave and fewer QSOs reach the log with a slip in the call) and each
  held to the coverage floor. The pile-up fixes that followed took the
  pile-up seeds to 3,000: Enter sends the caller you go on to your
  exchange rather than a TU that logs it at once, so fewer of its QSOs
  reach the log with a slip.
- **2b Sprint.** `myState`; US and Canadian places on the fallback shares;
  the Sprint serial fit; the deterministic parser; Sprint scoring, with the
  multiplier list checked against the current NCJ rules before it ships.
  Landed, but for that check: the NCJ rules could not be reached from the
  build's network, though excerpts of them a web search found agree (§3).
  With a third contest sharing the soak's seeds,
  and a Sprint's three-field exchange put wrong by a slip more often than
  a lone number, the soak's seeds rose by half to keep each kind of QSO
  over its floor.
- **2c Learner aids.** Hear a QSO with the perfect operator; presets; QRS.
  Landed: `hear_qsos` works the QSOs through in cw-core, and the app
  plays them as a preview on Listen and on the Character set card (§2);
  `ContestPreset` (§8); QRS in `contest/caller.rs` (§4.3). With them the
  hero's kept fields are stored as the run starts (§2), and the TU that
  logs after F3 charges no caller (§4.6).
- The Contest segment joined `CHAR_SET_MODES` right after 2a, at the
  user's request, so Contest can be tried while 2b and 2c are built; at
  600 px and below the six segments sit three and three.

The user can work single callers and pile-ups in WPX, Sprint and Simple QSO
on a phone or a desktop.

**Phase 3: the other three contests.** ARRL DX, CQ WW, Field Day; the
station fields and your side; region-filtered callers; zones, sections,
class and power shares; FCC counts in place of the fallback; both scripts
and a regenerated `callsign/prefixes.rs`. Every table line is checked
against the published rules and pinned before it ships.

- **3a CQ WW and ARRL DX.** Landed: zones and continents from cty.dat in a
  regenerated `callsign/prefixes.rs`, its call shapes unchanged; North
  America's zones by hand; the ARRL DX side; power shares; both rows,
  their scoring and their pills; `myZone`, `myPower` and LB. The soak
  shares its seeds among five contests now, each over its floor, and
  draws calls on both sides of ARRL DX, Labrador's among them. Its check
  that the clock never goes back now also passes the retries a failed send
  gives back when that ends a run past its limit, as the machine does
  (§4.2), which a new seed met. After review: a station is scored in the
  country it is at home in (`home_country`), so Alaska's and Canada's
  prefixes the table lacks score as their countries, and the US's islands
  the table misreads as the countries they are, or as none where it holds
  none (KH2); European Turkey is in Europe; ARRL DX
  gives nothing for a QSO on your own side; Guantanamo Bay's KG4 with two
  letters is DX and never drawn for a contest (§11.1, §11.2); your state
  settles your zone for a Canadian call whose digit names no place;
  callers' zones and power have their real-habits path (§11.6); Home asks
  for a zone in a field of its own; and the card and Your station say your
  side, your call's reading and your zone in the words §2, §3 and §8 give.
- **3b Field Day.** Landed: the Field Day row, its pill, its dialogue
  from the row and its simplification line; the 85 sections, YT read as
  TER; the class, section-by-place and DX shares counted by
  `scripts/contest-data.py`, which also reproduces 3a's power table
  exactly; the section a call tells by itself; a caller's class and
  section; the score of QSO points; `myClass` and `mySection` on Your
  station, with Home asking for the section. The FCC licence counts could
  not be fetched (§11.2), so the census shares stay; and ARRL's and RAC's
  lists could not be reached to settle Yukon's code, YT or YUK (§11.4),
  which stays open. The soak shares its
  seeds among all six contests, each over its floor, Field Day's stations
  with a section, none, DX or YT.

**Phase 4: realism and aids.**

- `contestHabits`: numbers as MRCE's callers send them (`real`); LIDs and
  QRM (`rough`). LIDs send a wrong digit then `EEEEE`, send their exchange
  early, or accept a wrong call 4% of the time (`Station.pas:509-521`,
  `DxOper.pas:651-671, 879-886`). Landed as slice 4a: the setting on the
  Band card and in the presets (§8), real numbers checked against
  `Station.pas` (§11.6), careless operators (§4.3) and QRM inside turns
  (`contest/qrm.rs`, §5.6); the soak runs callers of every habit.
- `contestShowCopy`, the screenshot's Preview Mode: each caller's text shows
  once its send ends. An item's stats copy is frozen when its text appears
  (received = the field text then; the reveal counts as a send); later
  edits count only for the log. The run is marked `contest:<kind>:a` and
  stored `assisted`: in every stat, greyed and unplaced on the board.
- The Runs card.
- Landed as slice 4b: Show copy on Session shape (§8) and in the RX line, a
  second after each send ends (§7), the copy kept as the fields held it
  then, read from their edits at your next key (§9.1), assisted runs on the
  board and in History (§10), and the Runs card under the Contest pill on
  Stats → Scores (§10); and, from a browser test on a small phone, Pause
  keeping its place while paused and Resume scrolling the run screen to its
  top (§2, §7).

**Phase 5: sound and Listen.** The receiver gate; flutter; a timed splice
for continuations; contest tour steps; adaptive caller speed from the Contest
copy-speed series; the paddles' field space.

- Adaptive caller speed was brought forward at the user's asking and has
  landed (2026-10-22, §15.2): `contestAdaptSpeed` on the Speed card, on as
  it ships, beside "Aim for" while it adapts and in place of group
  training's switch (§8); each newcomer's speed drawn by the call source
  from the copy-speed belief (`CallSource::next_speed`), started from the
  Contest series and taught the run's calls and numbers as each QSO is
  final (`CallSource::learn_speed`), each counted whole, a lost end a
  miss (`whole_evidence`), QRS slowing a caller from the speed it came
  with (§6.6); `adaptive_speed_path` reading the mode's switch
  (`adaptive_speed_on`), so the Sampling and copy-speed cards say how the
  callers come (§10), and Listen's stream of groups kept to an even draw
  (§2); and the soak's runs adapting on every other seed, every caller's
  speed held to the Speed card's ranges but for QRS under easy habits,
  which slows it by at most 22% (§13).
- The receiver gate, flutter and the timed splice landed as slice 5a, on
  both players and the recorder: without full break-in the receiver muted
  after its AGC while you send, nothing sent across your message counted
  as heard, and "Full break-in (QSK)" on the Tone and volume card, off as
  in MorseRunner (§5.3, §8); flutter on three callers in ten under the Band
  card's flutter, which Contester turns on (§5.5, §8); and a continuation
  spliced on at the start of your message's next character, each player
  starting it at that moment against the send it takes over from, no
  sooner than the player's lead and a release after the key, the lease,
  stall detection and retries kept (§5.2). The soak runs full break-in on
  half its seeds and flutter on two in five (§13). What could not be
  checked here: how the gate, a flutter and a splice sound by ear, on any
  player, and above all in a browser, whose gains, context clock and
  scheduling no test reaches, a stop cutting a send a continuation was to
  take over from among them; nor how long a real device's lead is, against
  the one the machine measures; the native mixer's are tested sample by
  sample, and the browser's gate schedule as numbers (`GateSchedule`), not
  as played.
- The contest tour and the paddles' field space landed as slice 5b: the
  guide in Contest mode, its button shown there, Start contest, the run
  screen's parts one at a time on desktop and touch, and Discard through
  `abort_run`, its Next never starting a run (§2); and a word space keyed
  on the paddles after a letter, six dits of quiet, pressing Space by
  the Space key's rule, the keyer's word gap a cw-core change (§4.5).

**Later, not planned:** Search & Pounce with the Sprint QSY rule, RIT,
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
| Your speed (operator feedback, 2026-10-22; was "Speed offset, for the run only") | a setting of its own, the callers' middle until set, kept as − / + leave it; the callers' speeds the Speed card's | an offset for the run that shifted later callers too | a fast runner keeps the speed it sends at, as MorseRunner's `Wpm`; a shift of the callers lasting only the run would leave your speed kept and theirs put back |
| ESM on a bad exchange | sends nothing, "Fix exchange" | a `?` | the fault is your typing; MRCE refuses it too (`Main.pas:1210-1216`) |
| Again | `?`, as MRCE's F7 | `K5X?` | partials go through Enter |
| Sends | repeats you drew: calls asked for, exchanges any key drew | every re-send | pile-ups and slow typists would pay for waiting; time charges waiting |
| A correction | no added send; the first call is the copy | an added send | one error is not punished twice |
| Lost copies | a call group and a missed group per field for each station engaged, as for a QSO logged before its exchange came | logged copies only; a call group alone | wiping a hard call, or walking away once the exchanges are out, must not raise accuracy, nor the board's points per group |
| Fields logged before they were heard | a missed group each | no group | logging early with a guess must not raise accuracy, nor the board's points per group |
| A send a pause you chose cut | owed on the caller's timeout | sent again a moment after Resume | a pause is no free `?`; a hidden page or failed sound is no choice, and does send it again |
| Esc on your message | the callers listening answer what they heard as garbage, at once | silence for a reply timeout | MRCE's callers do (`AbortSend`); the quiet was thinking time MRCE never gives |
| The station you work, waiting for your exchange, hears nothing from you | stops asking after 3 silent timeouts, stays, and takes it late; in any other worked state it leaves after 3 | leaving after 3 | MRCE's `osNeedNr` stays; a slow learner keeps the QSO. MRCE's others leave on the fourth; Dust keeps three |
| Enter on the call of a station that has gone: the one you work, or (as built, third browser test) any you heard that no station on the frequency would take, or a partial or a slip of it sent to nobody, once nobody calls | CQ, the fields cleared, the hint saying why; once someone calls, the fields cleared alone (a partial or a slip goes out as typed till nobody calls) | its call and your exchange | nobody would answer it, a pile-up waits on that QSO in silence, and a CQ over a caller only costs it |
| F3, then log | checked against the station F3 closed | a NIL | MRCE's log finds a station done with you (`Log.pas:745-755`) |
| Stored tokens | as keyed | canonical | Letters and Mistakes see what was heard |
| Copy speed | calls and numbers only | every group | closed lists are guessed, not copied |
| Board | per group within a kind; full runs ≥ 5 min | one Contest pill; 10 groups | exchange length would order it; 3 QSOs could top it |
| Labels | "Points"; "QSO pts · Mults · Score" | "Score" for both | two scores, two words |
| Keys | Tab as on any page; `;`; Alt+S, Alt+E | MRCE's Tab cycle, Insert | no trapped focus; screen readers keep Insert |
| Difficulty | presets setting the callers' speed only (2026-10-25; they wrote shared fields before) | a contest level | a level would own stations and tier; a preset writing them put back what the learner had set |
| Your call in a run | not stored; side, country, continent, zone are | stored | no score needs it |
| Numbers | yours cut (Sprint's serial and Simple QSO's 599 plain); callers' by habits, the Sprint's serials plain under every habit | always cut | what operators send; learners start plain; a Sprint serial cut to letters alone reads as a state |
| QRM (4a) | inside the turns on the air, carried over a key, never a station; one station at a time, starting as often as MRCE's | turns of its own; stations over one another, as MRCE's | the players hold one send; QRM is nothing to answer, so nothing to plan a turn for; one at a time, no station sends over itself |
| When Show copy shows a text (4b) | a second after its send ends | the moment it ends | a copy cut off at the end of a send misses the character heard last, typed a beat behind it; a later showing hides nothing |
| What the fields held when a text showed (4b) | read at the next key from the fields' own edits | a key or event of its own at the showing | the machine hears the fields only at a key; the edits tell it exactly, and a test's fields, with none, read as they are |
| A call left in the Call field as the next caller's text shows (4b) | no copy of that caller, a single caller's as a pile-up caller's: an empty copy | the field kept as its copy | the call is another station's (F3's, one gone), and one typed call would be two stations' copy, a mistake no ear made for Letters, Mistakes and the sampler |
| Pause while paused (4b) | kept in its place, inert | taken out; Resume in its place | End run slid into its place under a finger that taps twice; a second Resume would make a double tap resume |
| The Runs card (4b) | under the Contest pill on Scores, which shows once a run is kept | a tab or a card of its own | the board's contest and its runs side by side, and no new place to look |
| Space in Exchange (operator feedback) | back to Call where the exchange has one item to type, or what is typed reads whole, is nothing or ends in a space; else the space before the next item | a space, always | MorseRunner's and N1MM's space bar toggles the fields, so a call is put right with one key; a longer exchange still needs its spaces, and none is typed only to be dropped |
| Your sending (operator feedback) | a level of its own, off to the loudest in dB, -9 dB to start | a fixed monitor level | painfully loud against weak callers; -9 dB is level with an average caller as heard through the AGC |
| The paddles' sidetone (operator feedback; audit 2026-10-27) | a level of its own, half the limiter's knee, in every mode, turned down with the volume | the volume floor; your sending, or capped at it in a run with a floor that keeps it heard | the paddles type into the log and never transmit; one paddle should sound the same in every mode, and following your sending off would key in silence. The volume floor set how far the stations stand over the noise, so it moved the paddles 7 to 13 dB with it; a rig's sidetone level is its own. At the defaults the paddles sound about 3 dB under your messages and 4 dB under an average caller (§5.3) |
| Your call in TU (operator feedback; 2026-10-29) | every QSO unless you choose every third or never, with one caller or a pile-up, where the contest's TU leaves it; the callers call after every TU | every third, always; a pile-up's only | MorseRunner's station ID rate is a setting; its callers come on every TU, signed or not, so none waits for your call; a run station signs its TU on the air, one caller at a time or not |
| Callers' speed (2026-10-22) | each newcomer's drawn from the copy-speed belief within the Speed card's ranges, on as it ships, a switch of the contest's own; aiming at the Aim's share of characters right; QRS slowing from that speed | MorseRunner's keys alone; RufzXP's step up on a right call and down on a miss; group training's switch; a fixed contest target | the user asked for callers that speed up as calls are copied clean; Dust's speeds come from a posterior, so they move smoothly and as surely as the copies say; group training's switch stays off and its own; one aim for every adaptive feature, shown where it steers |
| Presets and the callers' speed (2026-10-22) | leave the switch alone | each set it | a preset says how fast callers may come; how they are drawn within that is yours, and toggling it keeps the preset |
| Assisted runs in the callers' belief (2026-10-22) | counted, as every session of the set is | left out | group training's belief is blind to sends; a copy kept when its text showed is what was heard by ear |
| A call's or number's lost end in the copy speed (2026-10-22) | a miss, live and in the history (`whole_evidence`) | the group's rule, its length's to answer for | no copy span learns from a run, so a busted call would read as clean, and faster callers, losing more ends, would push the speed up further |
| Asking again and the callers' speed (2026-10-22) | not counted: a copy counts as it ends | a repeat counted as a partial miss at the first send's speed | the series is blind to sends, as group training's is, and the record keeps the copy at its end, not what each send gave; asking costs rate and points, and a call still busted counts |
| Callers too fast to work (2026-10-22) | no evidence, a known limit | a speed-only miss kept in the run's log; a cap on the draw while the belief is wide | the record keeps only callers worked; a cap would put a rule of thumb over the posterior |
| Listen's stream in Contest (2026-10-22) | drawn evenly from the ranges, as before | following the callers' switch | the stream is no caller, and the switch speaks of callers |
| The receiver while you send (5a) | muted after the AGC, as MorseRunner's is with its QSK off; "Full break-in (QSK)" on Tone and volume a choice, off as it ships | always muted; never muted, as before | MorseRunner's `Qsk` is a saved choice, off by default; those who liked the band under their message keep it |
| Where full break-in sits (5a) | the Tone and volume card, after your sending | Band conditions; Your station | it says what you hear while you send, as your sending does; Your station holds your call and exchange, and its reset would take the switch with them |
| What the gate keys off (5a) | your message's span in the plan, from the turn's start to its end, your sending off or not | the dry tones | with your sending off there are no tones, and a rig transmits, and mutes, all the same |
| A caller sending across your message, muted (5a) | unheard, the whole send: no copy, repeat, speed, Enter or Show copy, RX counting it from your message's end; its cost to its caller spent | its tail after your message counted | nothing you cannot hear may count as heard, and a call heard in part is no call heard |
| Flutter (5a) | a switch of its own on the Band card in Contest mode, three callers in ten at 3–33 Hz, half to three quarters deep, over the fading or alone; Contester turns it on | MorseRunner's: a quicker fading in place of a station's, full depth, only with QSB on | the switch does what it says, and no caller is fluttered out of hearing |
| A continuation (5a) | a timed splice: the turn on the air plays out the character sounding, and any the player has begun, the next starts at the start of the next character, ridden over by then; with none left, half a word space after your message, the next message a word space after it | the next started at once from the character sounding, sent again whole; or started in the past, mid-character | no character twice and no gap; both players start a send at a moment ahead well, and your message is silent where they meet, so nothing clicks |
| How soon a continuation can take over (5a) | the player's lead, as the last turn started at once found it, and a send's release, after the key | the moment your message is at as it reaches the ear | a player plays ahead of the ear, so a character starting inside its lead has begun already, and taking over there sent its start twice; a lead measured on every turn keeps up with the device |
| What the turn on the air plays up to the splice (5a) | counted at the key, as planned | counted as it plays | everything is decided when you key; the turn on the air would otherwise stay beside the next for every count and timer. The cost: an Esc, a pause or the run's end before the splice, up to about a second after the key at 25 WPM, leaves counted a caller's send there that never sounded: with full break-in, any; without, only a reply starting in the word space before a splice after your message's last character |
| The contest guide's Next (5b) | moves on, never starting a run; Show me starts one when asked; away from the run screen its steps point the way to it | Next starting the run, as the first session's Next starts the session | a run's callers are sound nobody asked for, and a run is ten minutes of it, where a session's first group is a few seconds |
| The paddles' field space (5b) | six dits of quiet after a letter's last element, a dit short of a word space so one sent on time always ends it, then Space by its rule | the split between a letter space and a word space, five dits | a Space moves the cursor out of the call, so a hesitation mid-call must never make one |
| The guide when the mode changes (5b) | that mode's guide, from its first step | the step it was on | the two guides' steps differ, and a step of one is nothing to point at in the other |

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
6. The speed offset lasts one run. (Replaced on 2026-10-22 by 11 below:
   your speed is a setting of its own, kept from run to run.)
7. Builds older than phase 0 may drop logs but keep runs; the mark and
   backups recover them.
8. Data licences: asking AD1C about cty.dat facts and W7SST about the
   counted files is for the maintainer to do. Until they answer, the tables
   use facts and rounded counted shares only, as designed (11.8).

Decided with the user on 2026-10-22, from the feedback of a contester
running pile-ups at 235 QSOs an hour on the published app, which replaces
the design where it says so:

9. Space in the Exchange field goes back to the Call field, as MorseRunner's
   and N1MM's space bar toggles them, so going back to fix a call is one
   key: always where the exchange has one item to type; where it has more,
   once the items typed read whole, or the field is empty or ends in a
   space, and otherwise it types the space before the next item. It never
   types a space only for it to be dropped, and the cursor lands at the end
   of the call with nothing selected. The keypad's `␣ Field` follows it;
   the hint (on a wider screen) and the key bar say how to get back where
   a correction waits and Space does go back, and the key bar names what
   Space does at every keystroke; Tab moves as on any page (§4.5, §7).
10. Your sending has a volume of its own, "Your sending" on the Tone and
    volume card in Contest mode, from off to the old fixed level in dB, as
    MorseRunner's self-monitor level; it starts at -9 dB, level with an
    average caller as you hear one, not at the old maximum. Off leaves your
    messages silent and every time as it was (§5.3, §8). Whether the
    paddles' sidetone should follow it was not part of the feedback: it
    keeps the volume floor, and the level gap that leaves is a question
    for the user (§5.3, §15.1).
11. Your speed is a setting of its own, "Your speed" on the Speed card in
    Contest mode, as MorseRunner's `Wpm` is: the middle of the callers'
    range until you set it, so nothing changes until then; − / + moves it
    during a run and it is kept for the next run, and presets put it back
    to the middle of their range. − / + no longer shifts the callers, who
    keep the Speed card's ranges. The stored run's `my_wpm` records what
    you sent. This replaces 6 (§6.6, §8).
12. How often your TU signs with your call is a setting, "Your call in TU"
    on the contest card: every QSO, every third QSO (the default, as
    before) or never, in a pile-up, in every contest whose TU does not sign
    by its own rules (not the Sprint, whose TU always signs, nor Simple
    QSO, whose TU is its own). A single caller's TU never signs, as before.
    Callers call after your TU whether it signs or not, as MorseRunner's
    do, so a pile-up that never hears your call goes on as ever (§3, §8).
    Item 14 changes its default and takes it to a single caller.

Decided with the user on 2026-10-22, who asked for callers that speed up
as their calls are copied clean, as an option on by default:

13. "Adapt callers' speed to me" (`contestAdaptSpeed`) on the Speed card in
    Contest mode, on by default, a switch of its own, so group training's
    "Adapt speed to me" stays off and as it was. Each new caller's speed is
    drawn from the Bayesian copy-speed belief within the Speed card's
    ranges, aiming at the share of characters "Aim for" asks, shown beside
    the switch while it adapts; the belief starts each run from the
    Contest copy-speed series and learns within it from every call and
    number copied as its QSO is final, so clean calls speed callers up and
    misses slow them down, smoothly. Callers already calling keep their
    speed; QRS under easy habits slows a caller from the speed it came
    with; − and + move only your own speed; no preset touches the switch.
    Off, callers' speeds are drawn evenly from the ranges as before. Phase
    5's adaptive caller speed, brought forward (§6.6, §8, §10, §14); how
    each copy counts towards it is in §6.6 and §15.1.

Decided with the user on 2026-10-29, who found the card's example QSO
ending on a bare `TU` where a run station sends `TU` and its call:

14. Your TU signs with your call every QSO out of the box, with one caller
    at a time as in a pile-up: "Your call in TU" starts at Every QSO and
    is on the contest card for both run styles, Every third and Never
    still there to choose. A save from before reads as Every QSO once,
    since it wrote every third as `third` whether chosen or not; every
    third chosen now saves as `everyThird`. The example QSO ends
    `TU DL1ABC`. This replaces item 12's default, and its single caller
    that never signs (§3, §8).

## 16. Risks

1. **Phone height.** *Mitigation:* fixed one-line slots, the log below the
   fold, keys and spacing that shrink below 760 px (a 360×640 screen fits
   a run, and a session of groups, to the keypad's last row; a 320×568 one
   does not yet), a 360 px render test, a style test of the rule, a small
   phone.
2. **A continuation can still send part of a character twice** where the
   player's lead at the key is longer than the lead the machine last
   measured, as when the output device changes during a run or a start is
   slow, and a continuation asked for a moment the player has passed
   starts at once. *Mitigation:* the lead is measured afresh on every turn
   started at once (§5.2); listen on each player.
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
