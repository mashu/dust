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
- **Length** learns from **whole groups**, as it always has. Characters lost
  off the end of a group — everything after the last character typed — are
  the length's alone: you heard them, and the group was longer than you could
  hold.
- A group with **nothing typed** is the length's alone too. A send too fast to
  follow and a group too long to hold both leave a blank; a send that really
  is too fast shows in what does get typed.

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
ones. It counts as one observation with fractional success $y$:

$$
\log p(V, w \mid \text{data}) = \log p(V, w) + \sum_i \omega_i \left[y_i \log P_i + (1 - y_i)\log(1 - P_i)\right]
$$

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
  callsign at 30 WPM is not a two-letter Koch group at 30 WPM.

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
learner.

| Learner | Speed for 94% | Found | Sent at | Characters right | Span found | Length | Copied whole |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 90% at 20 WPM, span 5 | 18.6 | 19.5 | 18.5 | 94.5% | 4.1 | 3.0 | 76% |
| | | 18.9 | 18.4 | 93.6% | 4.6 | 3.2 | 70% |
| | | 18.9 | 18.5 | 92.4% | 4.1 | 3.1 | 69% |
| 90% at 30 WPM, span 7 | 27.5 | 27.2 | 26.5 | 93.9% | 5.5 | 3.8 | 72% |
| | | 29.2 | 27.7 | 94.9% | 6.1 | 4.3 | 67% |
| | | 27.2 | 26.0 | 94.7% | 6.2 | 3.9 | 72% |
| 90% at 40 WPM, span 4 | 37.7 | 37.1 | 35.8 | 93.8% | 3.5 | 2.6 | 74% |
| | | 38.5 | 37.2 | 95.2% | 3.5 | 2.7 | 70% |
| | | 36.5 | 35.2 | 94.3% | 4.0 | 3.0 | 64% |
| 90% at 30 WPM, span 7 at 20 WPM shrinking with speed | 27.5 | 28.7 | 27.3 | 95.1% | 5.0 | 3.2 | 77% |
| | | 28.0 | 27.3 | 93.4% | 5.4 | 3.5 | 74% |
| | | 27.9 | 27.1 | 92.8% | 5.6 | 3.5 | 67% |

The speed was found within about 5% of the truth every time. Characters came
out at 94% on average against a 94% target, and groups were copied whole 71%
of the time against 70%. Neither feature ran away with the other.

The span found is shorter than the learners' memory span, as it should be:
a group copied whole needs every character recognised as well as held, so
at 94% a character, the length copied whole is shorter than the length held.

The same learners with the speed learning from whole groups instead:

| Learner | Speed for 94% | Sent at | Characters right | Length | Copied whole |
| --- | --- | --- | --- | --- | --- |
| 90% at 20 WPM, span 5 | 18.6 | 10.3 | 98.1% | 4.0 | 70% |
| 90% at 30 WPM, span 7 | 27.5 | 10.2 | 98.4% | 5.6 | 70% |
| 90% at 40 WPM, span 4 | 37.7 | 10.2 | 98.5% | 3.0 | 73% |
| 90% at 30 WPM, span 7 at 20, shrinking | 27.5 | 10.3 | 98.0% | 8.0 | 70% |

(First seed shown; the other two are the same within 0.1 WPM.) The speed fell
to the bottom of the range for every learner.

Reproduce with `cargo test -p cw-core --release adaptive_speed_simulation --
--ignored --nocapture`.

## Where it is in the code

**cw-core**

- `speed.rs`: `SpeedBelief` (the speed's grid and curve on a `GridBelief`:
  history replay, learning from each group, `draw_speed`, `estimate`, and for
  the Sampling card `drawn_near` and `right_at`), `speed_evidence` (which
  characters count against the speed), `whole_evidence` (a contest's call or
  number, every character of it counting, those lost off its end too), and
  `SpeedPath` (the line through the speed settings).
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

- **Core:** the estimate moves the right way; a group cut short says nothing
  against the speed, and a blank says nothing at all; a simulated learner is
  found and copied near the target; a range too easy or too hard leans on the
  right end; draws stay in the range whatever the generator does; the speed
  settings are one line from slowest to fastest, Farnsworth included; history
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
