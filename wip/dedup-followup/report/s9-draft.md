## 9. Follow-up: one record store and one run lifecycle

*After §8 was merged (`2207d65`), on two branches of their own, `wip/dedup-store` and `wip/dedup-runtime`, each merged into this one.* The owner asked whether the two largest duplications of §2.3 had been fixed: the record-store algorithm written out for each store, and the two near-identical run runtimes. They had not. §7.4 had shared small parts of each and left the rest: the stores' protocols differed, and a `RunMachine` trait would move the lifecycle's code behind hooks rather than remove it, while one heartbeat for both would change when a run pauses. Each is now one implementation, behind a trait:

- **The record store:** one core over the files, a browser's database and the test store (§9.2), merged at `58133f1`. Its account was §7.7 at `061bfea` and `3b7f853`.
- **The run lifecycle:** one explicit state machine under a session of groups and a contest run (§9.3), merged at `984d718`.

Each went the same way:
1. The differences between the copies were mapped, and tests pinned those the plans found. A few in the stores, judged unreachable, were left to the switches, which declared them; the review after the switch found two more that nothing had pinned (§9.2).
2. Each difference that was a defect was fixed, test first, in the copy that had it, before any code moved. One was missed, and is fixed since (§9.5).
3. The code was moved in steps, each checked against the code before it. For the lifecycle, 4,000 seeded walks were compared line by line at each step, and no line changed. For the stores, differentials ran the old code beside the new and matched but for what each switch declared; the review found more that the switches had changed, each fixed or pinned since (§9.2).
4. A review of the result, its fixes, and a last check of those.

Neither made the code shorter. The moves and their reviews' fixes added 1,451 production lines, 753 of them code; the fixes made first added 421, 258 of them code (§9.4). §9.6 says what is left, and what is the owner's to decide.

### 9.1 What was decided

- **The record store.** Two designs were judged: a coroutine core that each store answers, and a generic core over a typed backend trait. The second was taken, with four things from the first: its stateless core, its exactness notes, the two fixes it found that the second had missed (made as `abe9586` and `66e652f`), and its order tests (scripted backends that log every call).
- **The run lifecycle.** Two designs were judged:
  - a `RunMachine` trait with a generic `LiveRun<M>`, which makes the lifecycle one implementation, for more code: the plan's sketch took the runtimes' 814 code lines to 880–900;
  - the shared steps as free functions with no trait, about as long as before (a sketch of it was 23 code lines shorter, 791 against 814), but leaving each runtime its own start, dispatch, effects, play and keep.

  The owner took the first: "I would implement the generic lifecycle. The additional 8–10% code is a reasonable trade-off if it provides" consistent initialization and termination, predictable state transitions, centralized error handling, better reproducibility of runs, easier testing and debugging, and separation between application logic and execution management. They asked for an explicit lifecycle state machine, a small abstraction rather than a framework of its own, and these invariants:
  - completed turns have timing information;
  - interrupted turns have no timing information;
  - interrupted turns remain in the execution history;
  - a paused run can be resumed without corrupting previous records.

  An interrupted turn is to be stored, explicitly marked incomplete, with no timing.

### 9.2 The record store as one core

**Now.** One core, `persist::record_store`, over a `Backend` trait with `async fn`s and no `Send` bound. Each store is a private view of itself that answers the core with its own reads and writes:
- the files (`Disk`, every future done as first polled on the worker);
- a browser's database (`InDb`, awaited on the page);
- the test store (`InMem`).

The core does the work:
- it reads the history whole, moves it in, and takes up what an older build changed;
- it makes changes, lists and reads;
- it answers the three asks of deletions, and tells of what does not read.

The notes of what was told are one set of rules (`record_store::notes`). A change is one set of steps (`record_store::steps`), and a page differential holds idb.js's transaction to them where each record of a change has a place of its own (`73b383c`). The protocols stay each store's own, behind the trait: durable steps and journals on the files, and one transaction with a compare-and-swap on the layout in a browser. Two things stay outside the core, and the trait is not as small as the plan hoped:
- each store's backup of the history (`stream_backup` on the files, `back_up` in a browser, the test store's `save_backup`): writing it once was optional in the plan, and was dropped for lack of time;
- a browser's page storage (`BlobHistory`), which keeps the history for a visit where the database will not open, will not take the history, or stops answering: it is the older builds' whole-by-key format;
- the `Backend` trait has 35 items (3 types, 2 constants, 30 functions, 24 of them `async`), where idb.rs's `Glue`, which the plan took to be nearly that trait, has 11.

**The differences, first.**
- Three commits came first and changed no behaviour: `1282a11` holds idb.js's `COMPATIBLE` and `BUMPS` to the Rust ones, `33a43ef` holds the notes of what was told to the names builds read them by, and `97e34cb` lets a test pin the wall clock, so that one sequence runs twice alike.
- 15 fixes followed, `e401d80` to `a56788e`, each test first, each making the copies agree where one was wrong, or both.
- A review of the pins and those 15 found seven problems, one medium: a move made over records stored a session a second time when the record that held it could not be read just then. Seven more fixes, `1073fc9` to `de8dd8d`, answer five of the problems: the medium one by `1073fc9`, with `04236c7` mending a wait for nothing that `0dac6c7` and `1073fc9` had brought in. The other two were gaps in the tests: no browser test held `e401d80`'s look at the tombstones by name, and nothing held the page script's own read of the note `33a43ef` pinned. `5af6787` and `1c6f0db` pin them. Of the review's races with another copy's Undo, the one in a browser is closed (`0e222e3`); on the files, whose Undo takes two steps, `de8dd8d` narrows it to the Undo's own two steps.
- Of the 22 fixes, 8 are in the files (`6f8460c` in the test store too), 5 in a browser, 6 in the test store, and 3 in `legacy.rs`, which the stores share (`e401d80`, `0642e2f`, `6563c5c`; `e401d80`'s fault was reached on the files alone).

**The switch.** The shared parts were moved out one at a time: the reads (`caa07a9`), the notes' rules (`e5a52e0`, `8bcf3d0`) and the change's steps (`54420dc`). The first of these was checked by the suites as they stood, the others by differentials of their own. The core was written beside them, and at first only its tests used it (`2590056`). Then each store was switched onto it (`95e0361`, `4834961`, `c01c5a9`). Each switch ran under a differential: the old code, pasted beside the new, ran on seeded sequences of 40 steps, and everything the two answered and held was compared and found the same, but for what each declared (on the files, set-aside names were compared without the second they were made in):

| Store | Sequences of 40 steps | Of them |
|---|---|---|
| Files | 2,000 | 11,929 loads, 16,063 changes, 8,018 listings, 4,062 reads, 3,923 + 4,044 + 4,061 asks of deletions, 4,039 of records still unread, 23,861 steps by another copy, an older build, other programs and a damaged disk |
| Browser | 2,000 | 9,662 opens, 20,050 changes, 4,026 changes by another tab, 6,434 listings, 4,013 reads, 3,282 + 3,161 + 3,196 asks of deletions, 1,591 backups, 19,757 steps outside |
| Test store | 4,000 | 19,166 loads, 40,014 changes, 14,355 listings, 7,945 reads, 6,405 + 6,379 + 6,407 asks of deletions, 6,392 of records still unread, 48,085 steps outside |

Each differential was also seen to fail on a mutation of the new code, then deleted with the old code. What each declared, and why it cannot be reached or is the test store's own:
- the files' `still_unread` reads the note of damage, where it had taken the note as there by its file (`95e0361`);
- a tombstone that cannot be written down fails a change, where a browser wrote `{}` (`4834961`);
- the test store writes its note of damage, empty, the first time it notes a record still unread; and, short of room, it lets a take-up's oldest records go after the read that tells of them, not before (`c01c5a9`).

One change went undeclared: a record that a change names twice now keeps its first tombstone in a browser too, as on the files, where idb.js had kept the last. `4834961`'s message says idb.js kept the first; `c96678b` pins both halves. No change the app makes names a record twice.

`9e39026` then fixed a bug the files and a browser shared (the test store's twin test passed before it as after): a record stored under the next name was read under the name asked, and in a browser noted under it. What every store must do with the history became a suite of cases (`persist::conformance`, from `a8f452b`): eight then, and 13 once the review's cases were added (`1f8fedb`, `5d03f77`, `c6df3c8`). The files run all 13, a browser 12 and the test store 10, each leaving out by name what it cannot hold. Each store's own tests hold what its protocol adds.

**The review** of the switch, by three reviewers, found eleven problems, two medium. Nine were places where the old code and the new answer alike: docs, tests, memory, code written twice, and what a store did alike before and after. No differential of the one against the other can find those. Of the two that changed what a store keeps, one needed two looks of one tab at once, which the differentials, taking one step at a time, never made; the other was the record that a change names twice (above). Each that changed what a store does was fixed test first; each in a doc or a test was pinned or corrected; and the code written twice was put in one place, with no change to what any store does:

- **A browser's take-up** was written over the layout as the tab last held it, not over the one it was worked out from (`4834961`; idb.rs had held the latter). Two looks of one tab run at once (news, the page shown again, the read-again timer, a backup, a restore). The earlier look, made second, replaced what the later took up: a deletion an older build made was left to take up again, and deleted the session once more after Undo put it back. A take-up is now written only over the layout it was worked out from (`0072574`).
- **The files' take-up** read at once every record it knew no session of (`95e0361`), where the files had read them one at a time: 4,000 records of some 25 kB that another copy stored raised the peak of memory by 100 MB. It reads them 256 at a time, on every store (`ae466da`).
- **The test store** answered every look at the history kept whole as a change, so the app's tests never ran the path the real stores take at nearly every look. It also noted the session of each record its move made, which no real store notes, so it took up what an older build stored while such a record could not be read (`c01c5a9`). It now looks and moves as they do (`78ec02d`, `b8ab8c5`).
- **Decisions written more than once:**
  - a browser's telling of every record still unread, in three places;
  - a take-up that took nothing, in two;
  - a check of the gate made twice in a browser;
  - four reads with default bodies that no store ran, which would have read nothing for a store that left one out.

  Each is in one place now, and the defaults stop where they are reached (`b4d5a71`).
- **Docs and tests:**
  - The files' look at the history kept whole was documented as doing the opposite of what it does (`890c8ae`).
  - The suite reached none of these: a tombstone that names no session, which left idb.rs's reading of one a browser holds as no text untested (`5d03f77`); two copies and an older build's look (`c6df3c8`); a deletion an older build made before the move (`0661bd4`); and the app's own change under the next name (`1f8fedb`). Its cases now hold them, and the near copies they replace in the stores' own tests are gone. The notes past the first telling, and a tombstone that cannot be read, stay in the files' and the test store's own tests, because the suite cannot hold a record open or make such a tombstone: that finding was answered in part.
  - The page script refuses a change two of whose records would go under one name, as one record twice, where every other store stores each once. No change the app makes holds such records, so the refusal is kept, said and pinned (`73b383c`).

Three commits came after `3b7f853` wrote the review into §7.7: two more of the review's fixes, and one that a last check of them asked for.
- `9344052` compiles for Unix alone a test helper that only Unix tests use, as the Windows lint requires.
- `4f8d642` says in the suite's own doc that each case is written once for every store, and that what a store's protocol adds is held by its own tests, where the doc had said all of it was tested once for all of them.
- The last check found one test regression: `c6df3c8`'s case let every store put off a take-up that the files' own test, which it replaced, required at the first listing. `aa6af38` holds the files to the first listing again, and the test store too, which no test had held to it before; only a browser, whose looks may race, is held to its next.

### 9.3 The run lifecycle as one state machine

**What was written twice.** `session_runtime.rs` and `contest_runtime.rs` each held:
- a start, building its kind's live record (the two record types, `LiveSession` and `LiveContest`, sat side by side in `state/live.rs`);
- the rule that finds a gap the app was not running in;
- the step that hands the machine what it hears;
- a dispatch, an effects loop and a timer;
- the task that plays a group or a turn;
- Resume, with its claim of the audio;
- the give-back as it pauses;
- an end: a keep and a let-go, written three times in all.

**The differences, first.** `dc661df` pinned, with seeded walks of both kinds, what a session and a run do after every action. A walk drives the calls the screens make: a start, taps and keys, Pause and Resume, Back, the page going out of sight, End and Discard. Time passes between them, held back now and then as a frozen phone holds it, and the fake player fails, stalls, runs late or skips. After every action the walk writes one line of what the app shows, plays, says, marks and stores. The commit's new tests pinned four of the 19 differences the plan's map lists, and the groups' side of a fifth; tests already there held the others, or the fixes' own tests did. They also pinned each of the owner's rules that already held. Then six fixes, test first:
- **`a1a7e90`:** every copy of a run left untimed says why, held up or sent again, as a group does. *Was:* untimed, with nothing to say so.
- **`f954ac7`:** nothing heard in a turn that a pause cut short is timed, and its copies say paused. *Was:* timed from what was heard in that turn, which a sound held up as the page hid could have made wrong.
- **`97c816e`:** a session's group that a pause cut part way through its send, kept by End while paused, is untimed and says paused. *Was:* its keys along the cut send were timed as if the send had played out.
- **`ef88e31`:** a pause writes into the mark what End would keep then. *Was:* the app stopping under a paused session lost the group the pause cut, and under a paused run the QSO it cut.
- **`88ac261`:** a paused run still hears the news of its own sends, as a paused session does. *Was:* a turn told late that it was held up kept its copies' times.
- **`c654e9d`:** Resume handed to a run as an event claims the audio first. *Was:* such a Resume would have carried on under the lease its pause gave back; no screen sends Resume that way, so no user could reach it.

Two reviewers of these found nine problems, seven of them distinct, one high and four medium. The high one, which both found: the mark a pause wrote went stale as the paused session or run heard the news of its own sends, so the app stopping while paused kept times from a send it knew was held up. What a paused machine hears that changes what End would keep now goes into its mark at once (`5d20794`). They also found:
- a session's group said "heldUp" on the web where a run's copy said "paused", because a hidden page's held-up send was heard after the pause, and the pins of it never hid the page (`13b94bf`);
- a run's copy left untimed by a pause gave no reason where the pause cut its first sending part way, or came before every key of it (`457b4a7`);
- the walks' hashes saw of a session stored only its accuracy and each group's text and timing class; the walks never checked the training screen; and they typed into, and sped up, a paused run (`cc90b44`, `865958b`, `c0b29fa`).

Two differences stay in what a pause untimes, one in each direction; the fix round chose not to make either one rule (`06c581d`):
- A run keeps no times from a turn that a pause cut short, not even from a copy keyed in full before the pause, as the owner asked for a paused run. A session, by its own rule from before this work, keeps timed a group answered in full before a pause that cut its send.
- In a turn that played out, a run's copy keyed across a pause keeps the times of the keys typed before the pause, while a session's answer left open by a pause keeps none. The plan offered the owner the session's rule for runs too (§9.6).

**The consolidation.** Nine steps, `b67c9f8` to `ffa0d14`:
1. each kind a `LiveRun` of its machine, with one gap rule;
2. the stage, with one give-back and one take-out;
3. one step;
4. one keep and one let-go;
5. one dispatch, effects loop and timer;
6. one play task;
7. one claim, one Resume, and one door for what the user does;
8. one go-live;
9. walks that check what must always hold.

After each step, 2,000 seeded walks of each kind were compared line by line with the base, `06c581d`: no line changed at any step. A step meant to change nothing has no test that fails first. Where a step added a rule or a check (the gap rule, the walks' stage check, the marks test), a mutation stood in: the gap rule made to see a run only when told, and `give_back` left not moving the stage or not writing the mark, each seen to fail the tests that hold it. Two more commits followed: `3307d10` brought docs/contest-mode.md up to date, and `f2e0714` put `RunMachine`'s items in the order of a run's life.

**What it is now:**
- **`state::live::LiveRun<M, O>`** holds the machine, its lease, its `Stage`, when it was last seen, the day it counts towards, its open mark, and what only its kind holds (`GroupsOwn`, `RunOwn`). `LiveSession` and `LiveContest` are its two kinds.
- **`Stage`** is `Running` (it holds the audio), `Paused` (the audio given back) or `Ending` (taken out to be kept or let go). Each move has one rule:
  - `gave_back`, from `Running` only;
  - `claimed`, from `Paused` only;
  - `ending`, once, from either.

  `hears` says what reaches each stage:
  - `Running`: what comes under its own current lease;
  - `Paused`: what comes under any lease of its own (End, Discard, typing, and the news of the send its pause cut);
  - `Ending`: nothing.
- **`live_run`** holds the lifecycle once, over `RunMachine`, a trait of 33 items that each kind implements: 7 types, 2 constants and 24 functions, 6 of them with default bodies. That is more than the "about ten hook methods" §7.4 first estimated, and close to the 36 the plan counted for this design; whether it is the small abstraction the owner asked for is the owner's to judge. It has:
  - one step for what a machine hears;
  - one dispatch, effects loop and timer;
  - one task that plays a group or a turn;
  - one claim of the audio, for a start and for Resume;
  - one give-back;
  - one end, with one keep and one let-go in one durability order;
  - one go-live.
- **`LiveRun::away`** is one rule, for both kinds, for a gap the app was not running in, and each kind is seen every `SEEN_EVERY_MS` (250 ms). How each is seen stays its own. A session is watched from its start, by its heartbeat and by everything it hears. A run is watched only from its screen's first tick, by its ticks alone. So when a phone freezes as a run starts, the run runs on with the gap counted as run time, where a session pauses from its start (`a_run_held_back_before_its_first_tick_runs_on`, `a_session_held_back_before_its_first_beat_pauses_from_its_start`). One heartbeat for both would change when a run pauses (§7.4); arming a run from its start is the owner's to decide (§9.6).

**The owner's invariants,** as the plan read them, and what holds each:
- **Completed turns keep their times.** A group answered in full keeps its characters' times, even if a pause comes after it, and so does a copy keyed along a turn that played out before the pause, unless news of the send says it was held up or sent again (the second case of `ending_a_session_paused_part_way_through_a_send_keeps_the_group_untimed`, and `a_turn_that_played_out_before_a_pause_keeps_its_times`).
- **Interrupted turns keep none.** A group or a run's copy left untimed by a pause, a send held up or a send made again says why (`paused`, `heldUp` or `sentAgain`, which 0.16.9 already reads), and holds no characters' times (`charTimes`), which is what the plan took "timing information" to mean. It keeps its answer time (`timeToCompleteMs`, with the pause left out), which the stored score charges and the average answer time counts, as for any group; taking it out of the score is the owner's to decide (§9.6). The mark of an interrupted turn is the reason `paused`, which every build keeps; a group a pause cut, resumed and then answered carries it too. After every action the walks check, in the history and in the mark, that a group or copy that says why it is untimed holds no characters' times (`97a77d5`).
- **Interrupted turns stay in the history.** End while paused keeps the QSO the pause cut, as lost, and the group the pause cut if anything was typed into it. A group cut before any key of it was typed is not stored: the plan chose this, as every build would count such a group as missed (`ending_a_session_paused_mid_group_keeps_the_group_begun`, `ending_a_run_paused_mid_qso_keeps_that_qso_as_lost`). The mark the pause writes holds the same, and is brought up to date as the paused machine hears news that changes it, so the app stopping while paused keeps it too (`ef88e31`, `5d20794`). The walks check that the mark holds the cut group last, as typed, while the session is paused with its answer begun (`25ef0bd`).
- **Resume corrupts nothing.** A pause and Resume change no group answered or QSO logged before the pause, field for field and time for time, whether Back, the page going out of sight or a gap away paused it (`resuming_a_session_changes_no_group_answered_before_its_pause`, `resuming_a_run_changes_no_qso_logged_before_its_pause`). Resume claims the audio afresh, only from `Paused`, before the machine hears it (`c654e9d`, `Stage::claimed`). After every action the walks check that the history only grows, that a session stored stays as it was stored, and that the stage is `Running` exactly while the audio is held and `Paused` exactly while the screen shows paused.

**What the owner asked of the lifecycle**, against what it does:
- *Consistent initialization and termination:* one go-live for both kinds, and one end, with one keep and one let-go in one durability order.
- *Predictable state transitions:* the stage moves by one rule each and is refused anywhere else, and the walks check it after every action.
- *Centralized error handling:* in part. A refused claim of the audio is handled in one place for a start and for Resume, and a send that fails in the one play task; what each machine does with a failure stays its own.
- *Reproducibility of runs:* the seeded walks drive both kinds as the screens do, check what must always hold after every action, and write their lines out (`DUST_LIFECYCLE`) for one build to be compared with another.
- *Easier testing and debugging:* the walks, and tests written once for both kinds (`either_kind_lives_one_life` and its siblings in `live_run.rs`).
- *Separation of application logic and execution management:* each machine, in cw-core, decides; `live_run` carries out what it says; each kind's hooks are its own.

**The review** of the consolidation, by three reviewers, found eleven problems, seven of them distinct, three medium. Once the walks' kept hashes were gone (`ffa0d14`), nothing pinned two rules of a session's away pause: that it takes the typing still waiting in the box before it pauses, and that Resume starts the heartbeat again. Nor did any check in the walks hold the owner's rules for a cut turn, which the hashes had held across every walk; the fixes' own tests held only the cases they were written for. Each now has a test or a walk check, seen to fail under a mutation of what it guards (`a8f8ca2`, `462e2ee`, `97a77d5`, `25ef0bd`). The low ones:
- `end` took what is live out of the app before it asked the stage, so a stage that could not end would have dropped it, its audio still held and its view still shown (`1d6fb74`);
- the trait's doc of which hooks run with what is live borrowed (`a4fc719`);
- docs/contest-mode.md, which named a function `fec64e7` removed (`c4f3e9d`);
- the walks' own 250 ms tick, now `SEEN_EVERY_MS` (`6a424c6`).

One was not taken: removing `Stage::Ending`, and with it having Resume return when its claim is refused, a case the reviewer found unreachable. The fix round kept `Ending` as the last stage of the lifecycle the owner asked for, and the end now asks it first (`1d6fb74`). The plan had named dropping it as its one lever on the code's size, and left that to the owner (§9.6).

A last check of the eight fixes found no regression. 8,000 walks, seeds 1 to 4,000 of each kind, pass every check, and give the same lines with `f2e0714`'s build as with `6a424c6`'s.

### 9.4 Code size

Production lines are those outside test-only files and outside every item compiled for tests alone; code lines are those that are not blank once comments are taken out. At each stage:

| Stage | Range | Lines | Code lines |
|---|---|---|---|
| The store: fixes first | `2207d65`..`1c6f0db` | +176 | +99 |
| The store: the consolidation | `1c6f0db`..`061bfea` | +1,082 | +640 |
| The store: the review's fixes | `061bfea`..`aa6af38` | +44 | +21 |
| The lifecycle: fixes first, in the app | `ab0cbf7`..`06c581d` | +91 | +63 |
| The lifecycle: fixes first, in cw-core | `ab0cbf7`..`06c581d` | +154 | +96 |
| The lifecycle: the consolidation and its review's fixes | `06c581d`..`6a424c6` | +325 | +92 |
| ⟦MOVEFIX-ROW⟧ |
| Both | `ab0cbf7`..⟦HEAD⟧ | ⟦BOTH⟧ |

⟦TOTALS⟧

Where the copies went:
- **The store.**
  - `files.rs` and `idb.rs` held 2,265 code lines before the consolidation, and hold 1,793 after the review (−472).
  - The core and its two rule modules hold 1,132: `record_store.rs` 890, `notes.rs` 159 and `steps.rs` 83. With them, the stores' halves and the core hold 2,925 code lines where the two copies held 2,265 (+29%).
  - The page script, `idb.js`, went from 521 lines to 557 (392 to 411 code lines): 542 (408) after the fixes made first. It is not in the table above.

  Every decision the review found made in more than one place is now in one, and each store's half is its own reads and writes. But the trait, its documented hooks and the core's shared types cost more than the copies they replaced.
- **The lifecycle.** The plan's 8–10% was reckoned on the runtimes and `state/live.rs` as they were at `ab0cbf7`: 814 code lines.
  - These files hold 964 code lines now: 18.4% more, against the 8–10% the owner accepted, where the plan's last estimate was about 11%.
  - Of the 150 lines more, 63 are the fixes made first (7.7 points), and 87 the consolidation and its review's fixes (10.7 points; 9.9% of the 877 the files held after the fixes). The consolidation added 5 more lines in `state/keep.rs` and `main.rs`.
  - The two runtimes went from 852 code lines to 594 (−258); `live_run.rs` holds 310, and `state/live.rs` 35 more, for the stage and the gap rule.
  - In all their lines, comments and blank lines included, the four files grew through the consolidation from 1,239 to 1,561 (+26%); in code and doc-comment lines, from 1,086 to 1,355 (+25%).

Test code grew by 6,868 lines in the app (4,069 of them the store's and 2,799 the lifecycle's), and by 477 in cw-core.

### 9.5 Found by this report's fact-check

⟦MOVEFIX⟧

### 9.6 What is left, and what is the owner's

**For the owner to decide.** Each stands as the plans left it:
1. **An interrupted group's answer time stays in the score**, with the pause left out. Taking it out would score those sessions afresh in this build only, so one session would rank differently in two builds.
2. **A pause now writes the mark** (`ef88e31`). A session or run the app stops under while paused is kept as End would have kept it, its interrupted group answered as it stood or its open QSO lost, and counted towards its level once.
3. **The lifecycle's size:** 18.4% more code lines, against the 8–10% accepted (§9.4). The plan's one lever, dropping `Stage::Ending`, would save about 4 lines.
4. **A run's copy keyed across a pause** keeps the times of the keys typed before the pause, in a turn that played out; the session's all-or-nothing rule for it is offered, not made (§9.3).
5. **A run's away check from its start:** a run is watched for a gap only from its screen's first tick, so a freeze as it starts counts as run time (§9.3).

**Left by the plans, with their reasons:**
- each store's own backup, and a browser's page storage outside the core (§9.2);
- `Store`, not split;
- each kind's liveness, kept its own (§9.3);
- on the files, an Undo another copy makes takes two steps, so one whose two steps span this copy's reads can still leave its session stored twice, at a take-up and at a move made again (`de8dd8d`, §9.5).

**To check by hand,** since here a browser's history ran only on stand-ins for its database, and the lifecycle only with the fake player: §6's Phase 0 lists them.

### 9.7 Verification

{{VERIFY}}

### 9.8 Effect on the plan (§6)

- **Item 8:** done: one lifecycle for both runtimes, with an explicit stage (§9.3). The heartbeat is still not shared, though the gap rule and its period are.
- **Item 9:** the core is done (§9.2), over a `Backend` trait of 35 items, 30 of them functions, rather than a small one. `Store` is not split.

### 9.9 Corrections to commit messages

These cannot be amended:
- `0642e2f` says the browser test's listing took nothing up. As committed, the test's `takenThrough` assertion fails first; the listing fails as stated once that assertion is set aside.
- `4834961` says idb.js kept the first tombstone of a record a change names twice. It kept the last (§9.2).
- `54420dc` says the page script is held to the change's steps. It is held to them only where each record of a change has a place of its own (`73b383c`).
- `a8f452b` says what every store must do is tested once, on all three stores alike. Near copies stayed in the stores' own tests until `5d03f77`, `c6df3c8` and `0661bd4`, and a browser and the test store run fewer cases than the files (§9.2).
- `dc661df` says its tests pin each way the two runtimes differ. They pin four of the plan's 19, and the groups' side of a fifth (§9.3).
- `35dd3fa` says a signal's readers see a write only once the call returns. They see it at once: a set changes the value as it is made, and only what is subscribed to the signal runs again later. What makes the moved order safe, and holds, is that `write_mark` and `mark_so_far` read neither signal.
- `5d134e2` says the runtimes no longer name the live record's fields. They no longer build it or name the preview, but they still read its fields: `machine` and `own` in both, `lease` in the run's `play_turn`, and `stage` in `hold_run`.
- `f2e0714` says every check passes as before. When it was written only fmt and clippy had run; the full verification after it bore it out. It also says the trait's doc names every hook called with what is live borrowed: the doc left out `over`, `of`, `of_mut` and `unboxed` (`a4fc719`).
- `58133f1` names §7.7 for the store's account: it is §9.2 now.
