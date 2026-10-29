# Adaptive speed

Status: implemented, off by default. Turn it on under Settings → Speed →
"Adapt speed to me".

## What it does

Without it, each group's speed is drawn evenly from the Character speed
range (and the Effective speed range, with Farnsworth spacing). With it on,
the trainer learns how fast you copy characters and sends each group near
the edge of that, the way RufzXP speeds up while you copy and slows down
when you miss:

- Copy the characters and the groups get faster. Miss them and they get
  slower.
- Speeds always stay inside the speed ranges you set. The ranges are the
  bounds the trainer works within.
- It works alongside adaptive group size: one learns from the characters,
  the other from whole groups (see below).
- With it off, nothing changes, down to the random draws.

The training screen shows each group's speed, with ▲ or ▼ when it moved
from the group before. Stats → Overview has a "Copy speed" card, and Stats →
Sampling a "Speed" card beside "Group length".

## Why not a staircase

RufzXP moves the speed a fixed step after each call. Dust already learns
your copy span with a Bayesian model of group length (see
[adaptive-group-size.md](adaptive-group-size.md)), and speed is the same
kind of thing: a quantity each group is drawn at, from a range you set. So
speed gets the same machinery — a posterior over a psychometric curve,
rebuilt from the history, with each speed drawn by Thompson sampling —
instead of a rule of its own. It moves the way a staircase does while it is
unsure, settles as it learns you, and needs no "start where you left off":
the history carries it.

## Two beliefs, two kinds of mistake

Group length and speed would both react to the same signal if both learned
from whole groups: was this group copied whole? Then a miss could mean "too
long" or "too fast", and nothing could tell which. Simulated (below), that
runs away: every group lost to its length is blamed on the speed too, and
the speed falls to the slowest the range allows.

So each belief learns from a different kind of mistake:

- **Speed** learns from **characters**: misheard characters, characters
  dropped inside a group, and characters typed that were never sent. That is
  recognition, which speed changes.
- **Length** learns from **whole groups**, as it always has.
- A group can **end early**, with characters lost off its end, two ways: it
  was longer than you could hold, or copy broke off at a character you did
  not recognise and the rest went by while you looked for it. One group
  cannot tell which. The rest of the lost end is a consequence either way,
  and is the length's alone. The character copy **broke off at**, the first
  one lost, counts against the speed unless adaptive group size can still
  answer for it, by sending the group shorter: while group size adapts and
  the group is longer than the shortest it draws, the whole end is the
  length's; at the shortest, or with group size fixed, nothing else answers
  for the break, so the speed does.
- A group with **nothing typed** speaks of no character. While group size
  adapts it is the length's alone: a send too fast to follow and a group too
  long to hold both leave a blank, and a send that really is too fast shows
  in what does get typed, now that the character copy broke off at counts.
  With group size fixed, nothing else learns from it, and it counts as its
  first character missed.

Before the character copy broke off at counted, a learner who stops typing
at the first miss showed the speed only characters copied right: the speed
ran to the top of the range, about 41 WPM for a learner who copies 90% at
20, with 2–3% of characters copied (see the simulation below).

Where an answer can be aligned more than one way at the same cost, the
reading that loses the most off the end is taken: "KE" for "KMRSU" is one
character wrong and three lost, not three dropped inside and the last one
wrong.

With two kinds of evidence there are two conditions to meet — characters
right at their target, groups whole at theirs — so the two settle at one
point instead of trading off along a line.

## The model

**Observation.** For each group: its speed $v$, and of the characters a
speed could have changed, the share $y$ copied right. The speed is the one
the score uses: $v = \sqrt{c \cdot e}$ for character speed $c$ and effective
speed $e$, so Farnsworth spacing counts.

**Each group counts once.** Its characters are not independent — a send too
fast loses several together — so a long group must not weigh like many short
ones. Each character counted is its share of the group as sent: a group of
$L$ characters with $r$ right of the $c$ counted adds $r/L$ to the hits and
$(c - r)/L$ to the misses. A group copied to its end counts once, with
fractional success $y = r/L$; one whose lost end is the length's counts for
as much of it as was heard. A miss that broke the copy off weighs as a
character misheard does, the characters it cost not counted: as a share of
what was counted instead, a group broken off at its second character came
out half wrong, and a learner who stops at a miss was held well below their
own speed.

$$
\log p(V, w \mid \text{data}) = \log p(V, w) + \sum_i \omega_i \left[h_i \log P_i + m_i \log(1 - P_i)\right]
$$

with $h_i$ and $m_i$ group $i$'s hits and misses.

**Psychometric curve,** on a log speed scale, so ten percent faster means
the same at 15 WPM as at 40:

$$
P(\text{right} \mid v, V, w) = (1 - \lambda)\,\sigma\!\left(\ell + \frac{\ln V - \ln v}{w}\right)
$$

- $V$, the **copy speed**: the speed at which you copy nine characters in
  ten. $\ell = \operatorname{logit}(0.9 / (1 - \lambda))$ is what puts it
  there.
- $w$, the **slope**: how sharply accuracy falls away as the speed rises.
- $\lambda = 0.02$, a fixed **lapse** rate.

**Posterior on a grid.** $V$ from 4 to about 80 WPM, two percent apart (151
points), and $w \in \{0.04, 0.08, 0.12, 0.2, 0.35\}$: 755 cells, kept exactly
as log weights.

- **Prior:** $\ln V \sim \mathcal N(\ln m, 0.4^2)$, $m$ the middle of the
  speeds the settings allow (on a log scale), and $w$ uniform.
- **History weights $\omega_i$:** as for the span, earlier evidence fades by
  $0.5^{1/10}$ a session.
- **Which sessions count:** only sessions in the same character-set mode. A
  callsign at 30 WPM is not a two-letter Koch group at 30 WPM. Their groups
  are read as the settings now read them: a lost end is the length's while
  group size adapts and the group could be shorter.

Nothing new is stored: the speed every group was sent at is already kept
with it. The belief is rebuilt when a session starts, faded once more, and
updated after every answer.

**Choosing the next speed.**

1. **Thompson draw.** Draw one cell $(V^*, w^*)$ from the posterior.
2. **Lean towards an aim.** Candidate speeds run across the range a percent
   apart. Each is weighted by its distance from an aim $a$ in log speed:
   85% from a narrow spread ($\sigma = 0.05$, about 5%) and 15% from a wider
   one ($\sigma = 0.2$, about a fifth) so the slope keeps being learned. The
   wider share stays near the aim rather than spreading over the whole range:
   a 48 WPM group in a run at 18 teaches little and costs the run.
3. **Calibrate the aim** by bisection, so the expected share of characters
   right over the speeds drawn is the target, for the same reason the span
   does (a spread around a point is copied less well than the point).
4. **Sample**, and keep it inside the range.

**Where on the speed settings.** The speed moves along one line from the
slowest corner of the settings to the fastest — character speed and spacing
together, each across its own range, on a log scale. Tied spacing moves with
the characters. A fixed character speed with a range of Effective speed
adapts the spacing alone, the way Farnsworth training does. The spacing is
never faster than the characters, so when Effective speed's slowest is
above Character speed's, the line starts with the spacing at the
characters' own speed and keeps it within its range as soon as they are as
fast as its slowest, as a station drawn evenly from the ranges is; the
speed along it is found by halving, as the copy speed only rises along it.

**Targets.** "Aim for" is shared with adaptive group size:

| Preset | Groups copied whole | Characters right |
| --- | --- | --- |
| Comfortable | 85% | 97% |
| Balanced (default) | 70% | 94% |
| Stretch | 50% | 90% |

Over a group of five, each character target about matches its group target:
0.97⁵ ≈ 0.86, 0.94⁵ ≈ 0.73, 0.90⁵ ≈ 0.59.

## Simulation

Synthetic learners recognise each character with the model's own curve and
hold a group to its end with a span curve of their own; a group not held is
lost from a random point on. Both adaptive features were on, at Balanced, with
Character speed 10–50 WPM and Group size 2–10. Each run is 400 groups; the
figures are over the second half, the estimates at the end, three seeds a
learner. Characters right are of those the speed counts.

| Learner | Speed for 94% | Found | Sent at | Characters right | Span found | Length | Copied whole |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 90% at 20 WPM, span 5 | 18.6 | 19.4 | 18.4 | 93.8% | 4.3 | 3.2 | 75% |
| | | 19.3 | 18.6 | 95.6% | 4.5 | 3.3 | 69% |
| | | 18.9 | 18.5 | 93.3% | 4.4 | 3.1 | 68% |
| 90% at 30 WPM, span 7 | 27.5 | 28.3 | 27.5 | 93.5% | 5.8 | 4.0 | 74% |
| | | 27.4 | 26.9 | 93.5% | 6.1 | 4.7 | 64% |
| | | 27.4 | 26.2 | 94.9% | 6.2 | 3.9 | 74% |
| 90% at 40 WPM, span 4 | 37.7 | 36.4 | 36.3 | 92.6% | 3.7 | 2.8 | 69% |
| | | 37.4 | 36.7 | 92.7% | 3.6 | 2.7 | 70% |
| | | 35.1 | 34.8 | 93.2% | 3.9 | 2.8 | 67% |
| 90% at 30 WPM, span 7 at 20 WPM shrinking with speed | 27.5 | 27.3 | 27.3 | 92.6% | 5.1 | 3.6 | 68% |
| | | 27.7 | 26.9 | 93.9% | 5.7 | 4.1 | 64% |
| | | 27.6 | 26.3 | 95.5% | 5.2 | 3.7 | 71% |

The speed was found within about 7% of the truth every time, and within 5%
in all but one run. Characters came out at 94% on average against a 94%
target, and groups were copied whole 70% of the time against 70%. Neither
feature ran away with the other. Over twenty seeds, the speed found
averaged within 1% of the truth for three of the learners, and 6% under it
for the one who holds only four characters: at the shortest groups the
range allows, some of the groups it drops count against the speed, as
nothing else can answer for them.

The span found is shorter than the learners' memory span, as it should be:
a group copied whole needs every character recognised as well as held, so
at 94% a character, the length copied whole is shorter than the length held.

The same learners with the speed learning from whole groups instead:

| Learner | Speed for 94% | Sent at | Characters right | Length | Copied whole |
| --- | --- | --- | --- | --- | --- |
| 90% at 20 WPM, span 5 | 18.6 | 10.2 | 98.4% | 4.0 | 72% |
| 90% at 30 WPM, span 7 | 27.5 | 10.2 | 98.4% | 5.6 | 70% |
| 90% at 40 WPM, span 4 | 37.7 | 10.2 | 98.0% | 3.0 | 73% |
| 90% at 30 WPM, span 7 at 20, shrinking | 27.5 | 10.3 | 98.0% | 8.0 | 70% |

(First seed shown; the other two are the same within 0.1 WPM.) The speed fell
to the bottom of the range for every learner.

Learners who stop typing at a character they miss, the rest of the group
going by, every time or half the time, with a span long enough never to
lose a group to its length. "Of all sent" is the share of every character
sent that was copied right. Group size adapting from 2 to 10, or drawn
evenly from 2 to 5:

| Learner | Group size | Speed for 94% | Sent at | Characters right | Of all sent | Length | Copied whole |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 90% at 20 WPM, stops at every miss | adapts | 18.3 | 20.8 | 95.3% | 77.9% | 2.4 | 74% |
| | | | 20.2 | 94.5% | 78.0% | 2.3 | 78% |
| | | | 20.0 | 94.3% | 75.5% | 2.3 | 74% |
| | 2–5 | 18.3 | 17.3 | 93.2% | 83.7% | 3.5 | 78% |
| | | | 17.6 | 93.3% | 82.6% | 3.5 | 80% |
| | | | 16.8 | 93.4% | 84.6% | 3.4 | 80% |
| 90% at 20 WPM, stops at half the misses | adapts | 18.3 | 19.2 | 96.6% | 81.4% | 3.2 | 73% |
| | | | 20.2 | 94.0% | 77.2% | 3.4 | 66% |
| | | | 19.1 | 95.6% | 80.8% | 3.2 | 70% |
| | 2–5 | 18.3 | 17.5 | 94.1% | 89.4% | 3.5 | 82% |
| | | | 18.1 | 93.1% | 86.9% | 3.5 | 79% |
| | | | 16.6 | 93.3% | 87.1% | 3.4 | 79% |

Before the character copy broke off at counted, the learner who stops at
every miss drove the speed to about 41 WPM either way, copying 2–3% of what
was sent. With group size adapting, the groups go short first and the speed
is held only at the shortest, so it settles a little fast, about 10% over;
with it fixed, a little slow.

Reproduce with `cargo test -p cw-core --release adaptive_speed_simulation --
--ignored --nocapture`.

## Auto level

Auto level counts a session towards a new character when its characters
were copied at the threshold (90% as it ships), and towards setting the
newest aside when they were not. Both adaptive features hold that share
steady by design. Adaptive speed keeps the characters it counts at its aim,
94% at Balanced; adaptive group size keeps groups whole at its aim, 70%,
losing the ends of the rest. A session's own accuracy, which counts every
character lost off an end and every group left blank, then sits near 80%,
under the threshold every time: judged on it, with either feature on, auto
level only ever lowered the level, and a learner who copies 90% at 40 WPM
went down to the first level as surely as one at 20.

So with either feature on, a session is judged on what the features leave
free (`auto_level.rs`):

- **Group size adapting:** the characters lost off the end of a group it can
  still send shorter are its to answer for, and are left out, as they are
  for the speed (above); a group with nothing typed counts as one character
  missed. What is judged is how well you copied the characters you reached,
  which the length does not hold steady.
- **Speed adapting:** wherever the speed can still move, it holds the share
  at its aim, so a session there says how fast you copy, not whether you are
  ready for another character, and counts neither way. Only once the speed
  is pressed against an end of the Character speed range, the middle of the
  session's groups within 5% of it, can it go no further, and the session is
  judged as one sent at a fixed speed is. At the top and at the threshold,
  you copy the characters you have at the fastest speed you asked for: a
  session towards a new character. At the bottom and short of it, even the
  slowest speed is too fast for them: a session towards setting the newest
  aside. At the bottom and at the threshold, the aim is beyond you at any
  speed the range allows, and the threshold is what is left to go by. The
  top of the range is the speed you train for, the Koch way: after each new
  character adaptive speed brings you back up to it, and the next comes once
  you copy the threshold there. With one speed set nothing adapts, and the
  threshold is judged as it always was.

Home's Auto level card says where a session counts while adaptive speed
steers, the toast that a level moved names the speed it moved at, and the
Overview's accuracy chart marks the threshold only where a session's own
accuracy is what it is held to.

**Simulation.** The learners are the ones above, with a level to be ready
for: each character has a copy speed of its own, a character just unlocked
copied at half the learner's speed and coming up to it as it is heard (two
thirds of the way in 25 hearings). They copy 90% at 20 WPM with a span of 7
at the start, at level 10 (eleven characters known). The **improving** one
gets 4% faster each session and learns every new character; the **stable**
one holds steady and can learn no character past the eleven; the
**struggling** one holds steady with four of the eleven beyond them. Auto
level is as it ships: 90%, two sessions above to level up, one below to
level down. Speed fixed at 15 WPM or adapting from 15 to 30; groups of two
or three, or adapting from two to eight. 36 sessions of 20 groups, seeds 3,
4 and 5 at each aim, Comfortable · Balanced · Stretch; "Before" judges every
session on its own accuracy, as auto level did.

| Learner | Adapting | Judged | Level after 12 sessions | after 36 | Mean level |
| --- | --- | --- | --- | --- | --- |
| improving | neither | | 10 12 14 · 10 12 14 · 10 12 14 | 17 20 26 · 17 20 26 · 17 20 26 | 14.6 |
| | group size | before | 6 6 5 · 1 1 1 · 1 1 1 | 4 1 2 · 2 1 1 · 1 1 1 | 2.9 |
| | | now | 14 14 13 · 16 16 16 · 16 16 14 | 26 26 25 · 28 28 28 · 28 28 26 | 18.2 |
| | speed | before | 13 11 9 · 11 9 9 · 8 8 9 | 25 18 16 · 21 16 16 · 17 14 16 | 11.9 |
| | | now | 10 10 9 · 10 10 10 · 10 10 10 | 14 15 15 · 16 14 13 · 18 15 15 | 11.2 |
| | both | before | 4 2 5 · 1 5 1 · 1 1 1 | 3 1 1 · 1 2 1 · 1 1 1 | 2.7 |
| | | now | 14 10 9 · 10 10 10 · 10 10 10 | 19 12 14 · 17 18 19 · 20 21 20 | 12.3 |
| stable | neither | | 10 11 10 · 10 11 10 · 10 11 10 | 10 11 9 · 10 11 9 · 10 11 9 | 9.8 |
| | group size | before | 3 9 9 · 1 1 1 · 1 1 1 | 1 6 2 · 2 1 1 · 1 1 1 | 3.5 |
| | | now | 10 11 11 · 11 11 10 · 10 10 10 | 11 10 10 · 10 10 10 · 11 11 10 | 10.3 |
| | speed | before | 10 8 9 · 9 8 9 · 1 6 4 | 11 10 8 · 9 8 7 · 1 2 1 | 6.8 |
| | | now | 10 9 10 · 10 9 9 · 10 10 10 | 11 11 8 · 9 10 8 · 10 10 10 | 9.7 |
| | both | before | 6 5 3 · 2 5 1 · 1 1 1 | 2 2 2 · 1 1 1 · 1 1 1 | 2.8 |
| | | now | 11 10 11 · 10 10 10 · 10 10 10 | 10 10 11 · 10 10 10 · 10 10 10 | 10.0 |
| struggling | neither | | 6 7 7 · 6 7 7 · 6 7 7 | 6 6 5 · 6 6 5 · 6 6 5 | 6.0 |
| | group size | before | 6 5 6 · 2 3 3 · 3 3 2 | 1 1 3 · 1 2 2 · 1 1 1 | 3.3 |
| | | now | 7 5 6 · 7 7 7 · 7 7 7 | 7 6 6 · 7 7 6 · 6 7 6 | 6.4 |
| | speed | before | 7 5 5 · 7 5 5 · 7 5 5 | 7 7 4 · 7 6 4 · 5 2 2 | 5.4 |
| | | now | 7 5 5 · 7 5 5 · 7 5 6 | 7 7 4 · 7 6 4 · 6 6 3 | 5.7 |
| | both | before | 5 4 3 · 3 4 6 · 1 1 1 | 2 3 1 · 1 1 1 · 1 1 1 | 3.0 |
| | | now | 7 6 5 · 7 6 6 · 7 6 7 | 7 6 7 · 7 6 6 · 6 6 6 | 6.4 |

Judged as before, adaptive group size took every learner down towards the
first level, the improving one too, and adaptive speed took the steady one
down at Stretch. Judged now, the improving learner levels up whatever
adapts, the stable one stays at its level, and the struggling one goes down
to the seven characters it knows, give or take one. With the speed
adapting, the improving learner's first new character waits until its
speed reaches the top of the range, about the twelfth session. The steady
learner wanders a level or two with nothing adapting as well: auto level as
it ships lowers the level after a single session under the threshold, and
these learners copy near it.

The design notes' own learners, whose every character is as easy as the
next, with both features on across 10–50 WPM over 40 sessions, went from
level 10 to level 1 at every aim; they now stay at level 10, sent between
the ends of the range.

Reproduce with `cargo test -p cw-core --release auto_level_simulation --
--ignored --nocapture`.

## Where it is in the code

**cw-core**

- `speed.rs`: `SpeedBelief` (the speed's grid and curve on a `GridBelief`:
  history replay, learning from each group, `draw_speed`, `estimate`, and for
  the Sampling card `drawn_near` and `right_at`), `speed_tally` and
  `speed_evidence` (which characters count against the speed: the character
  copy broke off at too, unless adaptive group size can still send the group
  shorter, which the belief knows as its `length_floor`), `whole_evidence` (a
  contest's call or number, every character of it counting, those lost off
  its end too), and `SpeedPath` (the line through the speed settings).
- `belief.rs` and `aim.rs`: the grid posterior and the aim found by
  bisection, shared with the span. The speed's `Leaning` keeps its wider
  share around the aim (`Spread::Around`).
- `alignment.rs`: `align_holding`, the alignment that loses the most off the
  end, which `speed_evidence` reads. It shares its table of edits with
  `align_group`.
- `sampling.rs`: `Sampler` holds a `SpeedBelief` beside its `SpanBelief`.
  `next_speed` draws a speed only when `adapts_speed` allows: the switch is
  on and the speed settings leave more than one speed. `learn` updates it
  whenever the group's speed was measured. The joint simulation lives in its
  tests. In Contest mode the same belief, started from the Contest
  copy-speed series, draws each new caller's speed (`CallSource::next_speed`)
  while the callers' own switch is on, and learns from the run's calls and
  numbers as each QSO is final (`CallSource::learn_speed`), each counted
  whole; `GroupSource::next_speed` draws none there, so Listen's stream of
  groups stays even. See §6.6 of `contest-mode.md`.
- `machine.rs`: `GroupSource::next_speed`, which by default chooses none,
  and `learn`, which takes the group as it was answered (`Answered`), with
  the speed it was actually played at. The machine keeps each group's chosen
  speed with it.
- `session.rs`: `planned_wpm` for the sender, `Group::answered` for the
  source, and `SessionView::speed` (`ChosenSpeed`) for the screen.
- `settings/mod.rs`: `adaptive_speed` (default `false`) in the Speed section;
  `Aim::chars_right` for the character targets. A contest's callers have a
  switch of their own in the same section, `contestAdaptSpeed` (default
  `true`, `settings/contest.rs`), which `adaptive_speed_on` reads in
  Contest mode, for `adaptive_speed_path` and the Speed card alike.
- `auto_level.rs`: `verdict`, how a session counts towards a level while
  either feature steers, `judged_tally`, the characters it is judged by, and
  `level_up_line`, where the accuracy chart marks the threshold.
  `learner_sim.rs` holds the simulated learners and the table above.
- `stats.rs`: `speed_history`, and `speed_forecast` for the Sampling card:
  round speeds across the range, with the chance of each and the share of
  characters copied right at it. With adaptive speed off the chances are the
  station's even draw, worked out by `station_speed_grid` in
  `timing/station.rs` from the same rules `resolve_station` draws by.

**App**

- `session_runtime.rs`: each group's station keeps its tone, strength and
  fist, but is sent at the chosen speed. The listen stream does the same.
- **Settings → Speed:** the switch. "Aim for" shows here while group size
  does not adapt, and this card's reset puts it back; once group size
  adapts, it is set under Session shape, for both (`aim_section`).
- **Training:** the speed chip, ▲ faster or ▼ slower than the group before.
- **Stats → Overview:** "Copy speed": the speed at the aim's share of
  characters, ± one standard deviation, over the last 30 sessions, and a chip
  saying whether adaptive speed is steering. With more than one character
  set in the history, a pill for each shows its own.
- **Stats → Sampling:** "Speed, WPM": round speeds across the range, each with
  its chance of coming next and the share of characters copied right at it.

## Tests

- **Core:** the estimate moves the right way; a group cut short counts the
  character it broke off at against the speed unless the length can still
  answer for it, and a blank says nothing while group size adapts; the
  history is read as the settings answer for lost ends; a learner who stops
  typing at a miss does not run the speed away, with group size adapting or
  not; a simulated learner is found and copied near the target; a range too
  easy or too hard leans on the right end; draws stay in the range whatever
  the generator does; the speed settings are one line from slowest to
  fastest, Farnsworth included; auto level follows an improving, a stable
  and a struggling learner with the speed and the length adapting or not,
  where judged as before it lowered them; history
  is replayed in order, older sessions count less, other modes are ignored; a
  stricter aim is a slower speed; both features together settle without
  either running away; the machine keeps a chosen speed with its group and
  learns at the speed played; old saves load with it off; the soak test runs
  with it on and off; the Sampling forecast matches the draw it describes,
  adaptive or the station's own.
- **App:** the switch is off by default, the aim shows in the right card and
  only once, one speed explains why there is nothing to adapt; a session sends
  every group and every repeat at its chosen speed, inside the range; the
  overview shows the copy speed card and the Sampling tab a speed card, for
  callsigns too; the chip says which way the speed moved.

## Open questions

1. **Character targets:** 97 / 94 / 90%, matched to the group targets over a
   group of five.
2. **One speed for every character set mode,** or per mode as now. Per mode
   keeps a callsign run from setting the speed of Koch groups.
3. **The span at speed.** The span still assumes one span at every speed. It
   follows the speed as the history fades; a span that depends on speed would
   follow it sooner, at the cost of a third parameter to learn.
