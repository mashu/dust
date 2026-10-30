# Adaptive group size

Status: implemented, off by default. Turn it on under Settings → Session
shape → "Adapt group size to me".

## What it does

Without it, a group's length is drawn evenly from the Group size range. With
it on, the trainer learns how long a group you can copy whole and draws
lengths near the edge of that:

- Copy groups whole and they get longer. Drop characters and they get
  shorter.
- Lengths always stay inside the Group size range you set. The range is the
  bounds the trainer works within.
- With it off, nothing changes, down to the random draws.

## The model

The skill being measured is your **copy span**: how many characters you can
hold and write down from one hearing. It is modelled the way adaptive
psychophysics models a threshold (QUEST, Watson & Pelli 1983; the Ψ method,
Kontsevich & Tyler 1999), with group length as the stimulus.

**Observation.** For each group, record its length $L$ and whether it was
copied whole, $y \in \{0, 1\}$: every character right and nothing extra
typed.

**Psychometric curve.**

$$
P(y = 1 \mid L, S, w) = (1 - \lambda)\,\sigma\!\left(\frac{S - L}{w}\right)
$$

- $S$, the **span**: the length you copy whole about half the time.
- $w$, the **spread**: how sharply success falls off past the span.
- $\lambda = 0.03$, a fixed **lapse** rate, so one slip on a short group does
  not collapse the estimate.
- $\sigma$ is the logistic function.

**Posterior on a grid.** $(S, w)$ lives on a fixed grid: $S \in [0.5, 20.5]$
in steps of 0.1, and $w \in \{0.4, 0.7, 1.0, 1.5, 2.5\}$, 1,005 cells. The
posterior is kept exactly as log weights, so there is no approximation and
nothing to diverge:

$$
\log p(S, w \mid \text{data}) = \log p(S, w) + \sum_i \omega_i \left[y_i \log P_i + (1 - y_i)\log(1 - P_i)\right]
$$

- **Prior:** $S \sim \mathcal N(\tfrac{\min + \max}{2}, 3^2)$ over the grid, and
  $w$ uniform. The range you chose is a fair first guess at your span. The
  history Stats shows starts from the middle of what its first session sent
  instead, so a change to the range does not redraw the past.
- **History weights $\omega_i$:** before each session, all earlier evidence is
  multiplied by $0.5^{1/10}$, so a session ten sessions back counts half as
  much. Groups in the current session count 1. The estimate follows you as
  you improve instead of being anchored by your first week. Only the newest
  400 sessions that count are read: one further back counts $0.5^{40}$, about
  a millionth of a millionth, which no figure shows, so a longer history
  costs a session start nothing more.
- **Which sessions count:** only sessions in the same character-set mode
  that have groups to read. Callsign mode is excluded, because a callsign's
  length comes from its structure, not a draw. A session with no groups
  neither adds evidence nor fades what came before it.

Nothing new is stored. The belief is rebuilt from the session history when a
session starts, the same way the character sampler's is, and faded once more
so the last stored session counts as one session back. The sampler's belief
in each character, and the Letters tab's mastery, fade the same way, though
over every session that copied a character, whatever it practised. It is then updated
after every answer during the session, a timed-out answer counting as a
miss. The app rebuilds all three ahead of the tap that starts a session,
once the practice or Listen screen is drawn and once a session is kept, and
keeps them until the history, or a setting they read, changes.

**Choosing the next length.**

1. **Thompson draw.** Draw one cell $(S^*, w^*)$ from the posterior. This
   explores while the posterior is wide and settles as it narrows.
2. **Lean towards an aim.** Weight each allowed length by its distance from
   an aim $a$, in characters, with a share $\varepsilon$ spread evenly over the
   $n$ allowed lengths so every length still turns up now and then:
   $$q_a(L) = \frac{\varepsilon}{n} + (1 - \varepsilon)\,\frac{e^{-(L - a)^2 / 2\sigma^2}}{\sum_{L'} e^{-(L' - a)^2 / 2\sigma^2}}$$
   with $\varepsilon = 0.1$ and $\sigma = 1$ character.
3. **Calibrate the aim.** Choose $a$ by bisection so the expected rate of
   groups copied whole is the target $t$:
   $$\sum_L q_a(L)\, P(y = 1 \mid L, S^*, w^*) = t$$
   Aiming at the single length that is copied whole $t$ of the time falls
   short. The curve bends down past that length faster than it climbs before
   it, so a spread of lengths around it is copied whole less often (Jensen's
   inequality). In simulation that version reached only 53–61% against a 70%
   target. If the whole range is too easy, the aim goes past the long end and
   the longest groups take nearly all the weight; if it's too hard, the
   shortest do.
4. **Sample.** Draw $L \sim q_a$.

**Targets.** The setting is called "Aim for":

| Preset | Groups copied whole |
| --- | --- |
| Comfortable | 85% |
| Balanced (default) | 70% |
| Stretch | 50% |

For reference, Wilson et al. 2019 (*The Eighty Five Percent Rule for optimal
learning*) put the best training accuracy near 85%. Counting a whole group
as one item, that is Comfortable.

**Cost.** $\ln P$ and $\ln(1 - P)$ for every cell and every length up to the
longest group the settings allow are computed once into a table, so an
update is 1,005 multiply-adds. When rebuilding from history, each session's
groups are first counted per length, so a session costs one update per
distinct length, not one per group. With adaptive length off, the Sampling
tab skips working out the draw chances, since every length is equally
likely. On 500 sessions in a native release build, replaying the history
takes about 4 ms, and so does the Sampling tab.

## Simulation

The policy was checked against synthetic learners whose true curve is the
model's own. The settings were Group size 2–10, and a single run of 200
groups with no fading between sessions. Estimates are from the first seed;
the whole-group rate is over the last 100 groups, for three seeds.

At the Balanced target (70%):

| Learner | After 20 groups | After 60 | After 200 | Copied whole |
| --- | --- | --- | --- | --- |
| span 4, spread 0.7 | 4.6 ± 0.5 | 3.9 ± 0.3 | 3.9 ± 0.1 | 63%, 64%, 79% |
| span 6, spread 0.8 | 6.1 ± 0.7 | 5.7 ± 0.3 | 5.9 ± 0.1 | 65%, 66%, 76% |
| span 9, spread 1.5 | 8.9 ± 0.5 | 8.4 ± 0.3 | 8.5 ± 0.2 | 62%, 64%, 76% |
| span 4 → 7, improving | 4.6 ± 0.5 | 4.3 ± 0.2 | 5.5 ± 0.2 | 81% |

Averaged over learners and seeds, the share copied whole was 83% at the
Comfortable target, 68% at Balanced and 48% at Stretch.

The improving learner is tracked with a lag, because one run has no
sessions to fade between. In the app, older sessions fade, which shortens
that lag.

## Where it is in the code

**cw-core**

- `belief.rs`: `GridBelief`, the posterior held exactly on a grid: a log
  prior, evidence that fades a session at a time, the Thompson draw and the
  estimate. `sessions_in_order` picks the sessions a belief is rebuilt from.
  Adaptive speed is built on it too.
- `aim.rs`: leaning a draw towards an aim, and finding the aim by bisection
  so that the draw as a whole meets the target. Shared with adaptive speed;
  each passes a `Leaning` of its own.
- `span.rs`: `SpanBelief`, the span's grid and curve on a `GridBelief`:
  history replay, learning from each answer, `draw_length`, `estimate` and
  `forecast`.
- `sampling.rs`:
  - `Sampler` holds a `SpanBelief` beside its `CharSamplingState`.
  - `next_group` asks the belief for a length only when `adaptive_bounds`
    allows: the switch is on, the range has more than one length, and the
    mode is not callsigns. Otherwise it keeps the even draw, unchanged.
  - `learn` takes the group as it was answered (`Answered`) and updates
    both the character beliefs and the span.
  - `aim_section` says which settings card shows the aim, and so which
    card's reset puts it back.
- `settings/mod.rs`: `adaptive_group_size` (default `false`) in the Session
  shape section, and `aim` (`Aim`: `Comfortable | Balanced | Stretch`,
  default `Balanced`, saved as `groupSizeTarget`). The aim is shared with
  adaptive speed. Saves without them load with the feature off.
- `stats.rs`: `span_history` and `group_length_forecast`.

**UI**

- **Settings → Session shape:**
  - The switch.
  - "Aim for", shown only while the switch is on.
  - With one group size, the switch explains why there is nothing to adapt.
  - In callsign mode the switch is hidden.
- **Stats → Overview:** a "Copy span" card. It shows the span with ± one
  standard deviation and its change over the last 30 sessions, as a line
  with an uncertainty band. A chip says whether adaptive length is steering.
  With more than one character set in the history, a pill for each shows its
  own span; choosing callsigns in the settings keeps the others on show.
  Past the longest or shortest group sent it says only "over" or "under"
  that length, and it calls a move up or down only past twice the first and
  last spreads together.
- **Stats → Sampling:** a "Group length" card. Each length gets its chance of
  coming next and how often a group that long is copied whole. Before any
  session of the set it says there is none to read.

## Tests

- **Core:**
  - The estimate moves the right way on successes and misses.
  - One slip doesn't collapse the span.
  - A whole copy needs nothing missed and nothing added.
  - A simulated learner is found within half a character, and is copied whole
    at 60–80% at the 70% target.
  - Each target is met exactly for a known curve.
  - A range that is too easy or too hard leans on the right end.
  - Draws never leave the range, even with a stuck generator.
  - History is replayed in time order, older sessions count less, and other
    modes and callsigns are ignored.
  - With the switch off, the draws are identical to before.
  - In a session, misses shorten the groups.
  - The settings reset covers both fields, and the aim goes back with the
    card that shows it.
  - A soak test runs random settings and random answers with adaptive length
    on.
- **UI:**
  - The switch is off by default.
  - The target appears only while the switch is on, and is stored.
  - One group size explains itself, and callsigns hide the switch.
  - The Overview shows the span card with or without adaptive length, and
    not for callsigns.
  - The Sampling tab shows each length, evenly weighted when adaptive length
    is off.

## Open questions

These were settled with the proposed defaults and can be revisited:

1. **Targets:** 85 / 70 / 50% of groups copied whole, with Balanced as the
   default.
2. **Speed:** one span for all speeds, with older sessions fading. A span per
   speed would need more data before it is trustworthy. Adaptive speed
   ([adaptive-speed.md](adaptive-speed.md)) moves the speed alongside, and
   learns from characters rather than whole groups, so the two do not pull
   against each other.
3. **Auto level:** judged on the characters you reached. Adaptive length
   keeps whole-group success at its aim, which holds a session's own accuracy
   under the threshold, and judged on it auto level only ever lowered the
   level. With adaptive length on, the characters lost off the end of a group
   it can still shorten are left out, and a session is judged on how well you
   copied the rest: see [Auto level](adaptive-speed.md#auto-level), with the
   simulation. A new character then shortens the groups until things
   settle. Each character's own tally leaves them out too, for Letters,
   What comes up next and how often it is drawn: counted as misses, they
   made every character look unmastered. Home's Auto level card says a
   session is judged so.
4. **What counts as success:** the whole group, which is what a span means.
   Judging per character would separate "didn't know the letter" from "lost
   the group", at the cost of a second model.
