# Dust: code review and plan

*November 2026. Branch `claude/admiring-rubin-6z9yr6`, from `ef0ab8f` (Release 0.16.9).*

## Summary

**Scope.** The whole first-party code base: about 78,000 lines of production Rust (44k in the app `src/`, 34k in the domain crate `crates/cw-core`), about 95,000 lines of tests, the JavaScript glue and the Android activity. `vendor/tao` (patched third-party) is out of scope.

**Verdict.**

- **Rust design.** The architecture is sound and idiomatic:
  - a pure, `unsafe`-free domain crate whose state machines return effects instead of doing I/O;
  - object-safe traits at the external boundaries (audio device, store, Google Drive, HTTP, sign-in), each with an in-memory fake (the page and the clock are reached through `#[cfg(test)]` seams instead);
  - distinct types for time and identity;
  - strict real-time discipline in the audio callback;
  - workspace-wide lints that encode lessons learned.

  The weak points are a few god objects (`ContestMachine`, `AppCtx`, the `Store` trait), stringly typed errors at subsystem boundaries, booleans standing in for states, and some layering inversions (§2).
- **Well written?** Mostly. Doc comments are dense and say what holds rather than what the code does, tests read as sentences and run on a virtual clock, and the history of each fix is in its commit message. Against it: a private vocabulary ("taken up", "told of", "put back") that a newcomer must learn, and functions that branch on platform × feature × test, which hides the path that ships (§2.4).
- **Monolithic?** Not at the function level: 39 of about 2,900 production functions exceed 100 lines, about half of them UI markup. A handful of files and types have absorbed too much and should be split (§2.2).
- **Redundant?** Textual duplication is low. Structural duplication remains in a few places, the largest being the record-store algorithm written twice (desktop files and IndexedDB) and the two near-identical run runtimes (§2.3).
- **Bugs, lock-ups, races.** The code base was well defended, but not free of them. This review found and fixed (§3, §4):
  - **7 crashes**: 3 reachable in release builds (the contest simulator in a browser, from Esc after the page fell behind; the closing, on Linux, of an audio stream whose ALSA worker had died of its own panic; a narrow sync race) and 4 latent or debug-only panics;
  - **5 fixes for a large or crafted file picked to restore**: in four of them the app could be stopped for want of memory (eight ways in all); in the fifth a browser page grew by up to a gigabyte it never gave back;
  - **3 permanent UI hangs** (one of them introduced by an earlier fix and caught before release) **and 4 UI freezes**;
  - **48 data-integrity defects** (sessions, settings or sync state lost, duplicated, reverted or hidden, and two Android leaks), 23 of them of Medium or High severity, mostly in sync, restore and persistence;
  - **a security issue**: the sync key and refresh token were written world-readable;
  - **35 other defects**, most of them behaviour users would notice.

  103 entries in all (a few group related defects), fixed in 146 commits, six of which change no behaviour; each is listed in §4. Nine of the entries are regressions that the review's own fixes introduced and later rounds caught, never released (the third hang, three of the data defects and five of the others), so 94 were defects of 0.16.9; five more entries name such a regression beside the defect it came from (fourteen in all, §4.4). Sync and persistence (with restore and backups) account for 47 of the 103.

  Every fix has a regression test that was seen to fail on the code before it, but for a few named in §4: paths only a browser takes (checked by the wasm32 build and its lints), Android's Rust and Kotlin (not compiled here at all), a desktop fallback no test reaches, a wait on the disk, an abort that takes gigabytes to reach, and a speed-up timed by hand. What is left needs a design change, a platform that cannot be tested here, or a product decision, or was judged too minor to risk a change; §5 lists each, with a recommended approach for all but the smallest (§5.4).

  What can still hold the UI thread: the history read at launch (no deadline, §5.1); settings, sync-state and mark writes, made with a file and folder fsync on the UI thread (a hung network home folder would block there); opening and probing the audio device; and the passphrase stretch, which a drive's keyring may now make at most about 100 times the app's own (256 MiB × 16 passes against 19 MiB × 2).

**Verification.** Before and after, on the pinned Rust 1.99.1:

| Suite | Before | After |
|---|---|---|
| `cw-core` tests | 1,320 passed | 1,359 passed |
| desktop app tests | 1,237 passed | 1,412 passed |
| silent (mobile) build tests | 1,129 passed | 1,284 passed |
| clippy `-D warnings`: desktop, web (wasm32), silent | clean | clean |
| `cargo fmt --check` | clean | clean |
| CI's Android and iOS type-checks, desktop lint on Windows and macOS, the web site's build and browser checks | not run here | not run here: they must pass in CI before this branch is merged |
| the 11 soak properties of `cw-core` over seed ranges far past their defaults (100 to 9,000 seeds per property per range)¹ | 1 crash (5 ranges of 0.16.9) and 1 invariant violation (a second soak of 7 ranges after round 1, reproduced on 0.16.9) | over 7 ranges (3 of them new): no invariant violated; coverage floors missed on 3 (test-only, §5) |

¹ Run on the fixed `cw-core`, which has changed since only by a test and a doc comment. The ranges were chosen with a local patch that offsets the seeds (`SOAK_BASE`); it is not in the branch (§6, item 18).

## 1. How the review was done

1. **Baseline.** Built and tested the configurations of CI's Linux job (desktop tests, web/wasm clippy, mobile-silent) on the pinned toolchain; CI's Android, iOS, Windows and macOS clippy jobs cannot run here. Ran the desktop suite four times with fresh random seeds to look for flaky, timing-dependent tests; there were none.
2. **Subsystem reviews.** Ten independent reviews: audio, persistence, sync, app state and runtimes, two for the UI, and four for `cw-core` (contest, DSP/keyer/timing, sessions/settings, sampling/analytics). Each was asked for crashes, hangs, races, logic errors and design problems, and to verify reachability before reporting.
3. **Bug-class sweeps.** Ten sweeps across the whole code base, one per bug class:
   - byte-offset string slicing;
   - integer over/underflow;
   - float hazards;
   - panicking collection operations;
   - signal/RefCell borrow conflicts;
   - async task lifecycle;
   - threads, locks and blocking;
   - robustness of stored and network data;
   - clocks;
   - FFI/JNI/JS/unsafe.

   A second, skeptical reviewer then tried to refute every finding: 26 findings, 23 confirmed.
4. **Dynamic testing.** Ran the randomized soak tests over five fresh seed ranges far beyond their defaults, which found the contest crash. A second soak over seven ranges, after the first round of fixes, found a contest-log invariant violation that the released code has too. The final soak ran on the fixed code over seven ranges, three of them new.
5. **Fixes, in five rounds.** Each fix was made test-first where its code can run here (§4 names the exceptions). Each round's fixes were then reviewed by an independent reviewer who could reject a commit: ten commits were rejected, nine of them redone in the next round and one left out (§5.3), and the reviews of each round found the follow-ups the next one fixed. Fourteen regressions that earlier fixes introduced were caught this way before release; §4.4 names each. Each round's fixes, once merged, were verified in an **isolated** build directory before they were pushed: the full suites and the three clippy configurations. (Once, two round-1 sets merged cleanly but did not build together until `914c88e`.)

   One process lesson is worth recording. Separate git worktrees of one workspace **must not share a cargo target directory**. Cargo names a workspace member's outputs the same in every checkout and decides freshness by file times. So one checkout can silently run or link another's build. That happened once here, was caught, and every claim made under it was re-verified in isolation.

## 2. Architecture assessment

### 2.1 Shape of the system

| Layer | Where | Role | Verdict |
|---|---|---|---|
| Domain core | `crates/cw-core` (≈34k production lines, `#![forbid(unsafe_code)]`, no I/O) | Koch/curriculum logic, sampling, scoring, the group-session machine, the contest simulator, the receiver/band DSP, the keyer, Morse timing | **Strong.** A *functional core*: state machines take events and return effect lists (`SessionMachine::apply`, `ContestMachine::apply`) instead of doing I/O ("sans-IO"). Deterministic RNG streams (`SplitMix64` forks per purpose) make a run replayable from its seed (the tests pin seeds; the app does not keep them), which the randomized soak tests exploit. |
| App shell | `src/` (≈44k production lines) | Dioxus UI, runtimes that execute the machines' effects, audio backends, persistence, sync | **Good, with a few overgrown modules** (below). An *imperative shell*: `session_runtime.rs`/`contest_runtime.rs` turn effects into timers, audio sends and stores. |
| Boundaries | traits `MorseBackend`, `Store`, `Remote`, `Http`, `Auth`; cw-core's `CallSource` | The audio device, the store, Google Drive, HTTP and sign-in are each behind an object-safe trait, all small but `Store` (36 methods); contest callers come through cw-core's `CallSource`. The page and the clock are reached directly, through `#[cfg(test)]` hooks and Tokio's paused clock in tests | **Strong for the traits; weak for the page.** Every trait has an in-memory fake (`audio::fake::Recorder`, `MemoryStore`, the fake Drive and sign-in), which is why the app suite runs 1.2k tests headlessly on a virtual clock. But the page and file reading are reached through about 77 `#[cfg(test)]` branches inside production functions, so the tests run a different branch from the one that ships (the reason one fix, `5cde976`, has no test). A `Page` trait with a fake, as `MorseBackend` has, would close it. |

Rust patterns the code uses well:

- **Effects as data / sans-IO state machines.** Domain machines never sleep, play or store; they return `Vec<ContestEffect>`/`Vec<SessionEffect>`. Timers carry ids, so a stale timer is ignored rather than cancelled.
- **Distinct types** for things that must not be confused: the newtypes `MonoMs` (saturating monotonic time) and `SessionId`, and the key types `RecordId` (order, nonce) and `SessionKey` (timestamp, date).
- **Capability-scoped async.** Tasks that outlive a screen use `spawn_forever` in the root scope. Every runtime task re-checks its `Lease` after each `.await`, so news from an ended or replaced session is dropped.
- **Real-time discipline in the audio callback.** It uses `try_lock` (never waits), the sidetone is atomics only, buffers are pre-sized, frees are deferred off the callback (`Swept`), and `catch_unwind` around rendering turns a DSP panic into a reported failure instead of a dead process.
- **Forward-compatible persistence, for keys.** Unknown keys are carried: a later build's settings in `TrainingSettings::unread`, a stored session's (and its groups' and timings') in `UnreadKeys`. Contest logs are kept as raw JSON with a strict typed reader. The desktop store writes its files atomically (unique temp file, fsync, rename, and on Unix a directory fsync); session marks are renamed into place without an fsync. An unknown *value* under a known key is not carried: it reads as this build's default (by design for `FilterShape` and `AgcDecay`, and by dropping the key for strict enums such as `CharSetMode`), and the next save writes that default over a later build's choice (§5.1).
- **Lints as policy.** `await_holding_refcell_ref`, `let_underscore_future` and `redundant_clone` are denied workspace-wide, and `unsafe_op_in_unsafe_fn` is denied. `unsafe` is confined to FFI (JNI, AVAudioSession), the desktop GTK menu and setting WebKitGTK's environment at process start (`linux_webview::prepare`), tests aside, each with a stated safety argument.

### 2.2 Is it monolithic?

Not in the function sense: of about 2,900 production functions in 0.16.9 (test modules and test-only files left out), 39 exceed 100 lines and 108 exceed 60. About half of the longest are Dioxus components, whose `rsx!` markup is naturally long; the rest are domain and runtime routines (`Caller::hear` 282 lines, `ContestMachine::count_heard` 253, `play` 196, `send` 195, `sync_once` 153, `Agc::next_gain` 132). The structural problems are a handful of **types and files that have absorbed too many responsibilities**:

| Unit | Size | Responsibilities mixed | Suggested split |
|---|---|---|---|
| `cw-core/src/contest/machine.rs` | 14.3k lines (≈5k production, one `impl ContestMachine` with 99 methods) | run state, air/turn planning, hearing/counting, logging, Esc undo, view | `machine/{mod, air, plan, heard, log, undo, view}.rs`. Tests (9k lines) go to `machine/tests/*.rs` by topic |
| `AppCtx` (`src/state.rs`) | 27 fields, 44 methods (≈1.3k lines of a `state.rs` of ≈1.7k production lines) | settings storage and merge, session marks, history sync with the store, delete/undo, backup/restore/undo-restore, settings-card undo, toasts | sub-contexts `HistoryCtx`, `SettingsCtx`, `MarksCtx`, `RestoreCtx`. Keep `AppCtx` as a thin bundle of them |
| `Store` trait (`src/persist/mod.rs`) | ~36 methods | flags, settings, counters, backups, marks, history, sync state | focused traits (`SettingsStore`, `HistoryStore`, `MarkStore`, `SyncStateStore`) |
| `cw-core/src/band/agc.rs` | `Agc` has 34 fields; `next_gain` 132 lines | detector, limiter, hang, ceiling and look-ahead in one routine | `TwoWindow` (used 3×), `DelayLine` (used 3× across crates), then detect/limit/hang steps |
| `src/ui/tour.rs`, `paddle.rs`, `widgets.rs`, `settings_charset.rs` | 1.2–1.7k production lines each (1.5–2.3k with tests) | data + automation + DOM scripts + components; widgets is a grab-bag of formatting, icons and fields | one module per concern: data, automation, page scripts and components apart |

These sizes are those of 0.16.9. This review's fixes added roughly 3.5k production lines, mostly where this section already saw too much: `AppCtx` now has 31 fields and 52 methods, `sync/ctx.rs` grew by half and `audio/native.rs` by a third. The hierarchy is also flat: cw-core has 31 top-level modules, its analytics among them, and `src/ui` holds about 50 files in one folder. Sync's logic (plan, pack, seal, state, run, drive, remote, http) imports only the store and cw-core, and only `sync/ctx.rs` reaches Dioxus and the app's state; the core of persistence (record, files, backup, legacy) needs only cw-core and the clock. A storage-and-sync crate would make the compiler enforce the layering §2.4 asks for.

Inline test modules are about half of most large files (`files.rs` 1.3k of 2.5k lines, `desktop.rs` 1.1k of 2.0k, `machine.rs` 9k of 14k). Moving them to sibling `tests.rs` files, as `band/` and `persist/idb/` already do, would roughly halve what a reader has to scroll past.

### 2.3 Redundancy

Literal copy-paste is rare (a clone scan over 6-line windows found only ~40 duplicated windows in 78k production lines). The duplication is **structural**: the same algorithm written twice.

| Duplicated logic | Locations | Consolidation |
|---|---|---|
| Record-store algorithm (`KeyOnly`, `keyed`, `taken_up`, `not_moved_yet`, `tombstoned_keys`, `tell`, `live_keys`, `import`, `migrate`/`move_in`, `legacy_deleted`) | `persist/files.rs` ↔ `persist/idb.rs` (10 pairs) | one generic core over a backend trait. idb's `Glue` is nearly that trait already. One divergence between the two copies was itself a bug (fixed in `8855056`) |
| Run lifecycle (start, resume, away-gap detection, timer spawning, play, end, progress mark) | `session_runtime.rs` ↔ `contest_runtime.rs` (`HEARTBEAT_GAP_MS` is even defined twice) | a `RunMachine` trait plus a generic `LiveRun<M>`. It would also unify liveness: groups use a root heartbeat, while runs depend on a mounted component's `use_ticks` |
| Band sync and continuation timing | `audio/native.rs` ↔ `audio/web/mod.rs` | `sync_band(&mut Mixer, …)` and one continuation-timing helper |
| Grid-belief replay scaffolding | `cw-core/src/span.rs` ↔ `speed.rs` | a `GridModel` trait with default `from_history`/`for_session`/`for_newest` |
| Percent formatting | 7 functions across `ui/` | one `format::percent*` family |
| Page-listener boilerplate (eval install, recv loop, global slot, uninstall) | 6 copies (4 in `ui/`, plus `page_hidden.rs` and `system_back.rs`) | `use_page_listener(slot, script, on_msg)` |
| Focus-by-id via eval | 5 copies | `focus_by_id(id, at_end)` |
| "Last mounted wins" registration | `use_lent`, `AnswerOutbox::use_entry`, `ContestEntry::use_letters` | a small `Lender<T>` |
| Sidetone burst constants | `audio/mixer/sidetone.rs` ↔ `audio/shape.rs` | one definition (the module header says that is its purpose) |
| A group's copy speed | `session.rs` `points()` (with a fallback of its own), `auto_level.rs`'s `group_speed` and `speed.rs` (sharing another) | `SessionResult::group_speed(i)` |
| `QsoRecord` construction | `worked_entry`, `nil_entry`, `lose` | one constructor |
| JNI call scaffolding | `persist/android.rs` (4 copies) ↔ `sync/auth/android.rs` (`with_activity`) | one `with_activity` |
| One restore at a time | `AppCtx`'s queue (`41eb9f1`) ↔ the backup card's wait for any restore under way | keep one; the card never queues, so the queue runs only in tests |

Dead code was not swept for as such; two items surfaced: `BandSource::fill` and `board_place` are used only by tests (`pub mod band` is exempt from the dead-code check cw-core otherwise relies on).

### 2.4 Abstraction and hierarchy

- **Layering inversions.** Domain and runtime code reaches up into the UI layer for words: `state.rs → ui::format`, `session_runtime.rs → ui::auto_level::change_message`, `sync/ctx.rs → ui::format`. Production code has top-level module cycles `state↔ui`, `state↔sync`, `state↔session_runtime`, `state↔contest_runtime`, `session_runtime↔contest_runtime`, `session_runtime↔ui`, `contest_runtime↔ui`, `sync↔ui`, `playback↔state` and `app↔page_closed`. Moving `ui::format` to a neutral `text` module, and having state return typed outcomes that the UI words, removes the cycles through the UI's words (`state↔ui`, `sync↔ui`, and `session_runtime↔ui` if `change_message` moves too). The others need the context split below (`AppCtx` holds `SyncCtx`, the runtimes take `AppCtx`).
- **Stringly-typed errors.** There are 41 `Result<_, String>` signatures in the app, several at subsystem boundaries (`MorseBackend::start`, `Http::send`, `Store::write_settings`, `HistorySaved::Failed(String)`, …). The few real error types (`AudioError`, `RemoteError`, `AuthError`, `JoinError`, `NotSaved`) mostly end in a string catch-all (`Failed(String)`, or `Device(String)` for `AudioError`). Transient failures (timeout, 5xx, device busy) cannot be told from permanent ones, so the retries that exist (a desktop history write after 100 and 400 ms, an audio send three times, unsaved history every minute) retry every failure alike, and sync retries a failed request only once, with a fresh token, after a refused sign-in: a timeout, a 429 or a 5xx is not retried. Small error enums with `Display` would fix that.
- **Booleans that are states.** Examples: `mark_running(run: bool)`, `OpenSession.run`, `Toast.long`, the four flags of `Tour`, `WaitFlags` (five bools plus options whose precedence lives in a function), `CallSpec.contest`, `Replying` (4 bools), `Caller` (~13 bools mixing fixed traits with QSO progress), and phase-specific fields of `SessionMachine` (`resume`, `countdown_to`, `missed`, …) that are valid only in one phase. Moving them into enum variants (`Paused { resume, since, why }`, …) makes the invalid combinations unrepresentable.
- **Indices as identity.** Contest stations are identified by their index in a `Vec` that Esc truncates. That is the root cause of the crash fixed in this review (C-1). A stable `StationId`, with "withdrawn" as a state rather than a truncation, would remove the class of bug.
- **Polling for flags.** About 32 waits loop on `sleep_ms(POLL_MS)` until a flag clears (`while ctx.restore_under_way() { … }`). Each one this review touched ends on every path, but the class is fragile: a flag never cleared is a wait forever, which is what the "Reading…" and "Syncing…" for good bugs fixed in `105ab0e`, `ed1baa0` and `fee1508` were. A notify or a channel would remove it.
- **Sync's design attracts defects.** Sync and persistence hold 47 of the 103 entries in §4. Each copy of the app writes its whole `SyncState`, and deletions and restores are tracked in hand-kept notes (`applied`, `revived`) whose stamps must stay consistent across devices, copies and builds: every rule added for one ordering of events exposed another. A pure planner over (drive, here, notes), soaked with seeded random event orders on several simulated devices, as the contest simulator is, would find these before users do (§6).
- **Two sources of truth.** The live run is in `Rc<RefCell<Option<Live>>>` and mirrored into `runtime`/`contest` signals. Each change deep-compares and clones the whole `GroupSession`. A run context that owns the machine and exposes a derived view would remove the mirror.
- **Domain logic in the UI.** `forecast::standing/meets`, the Finding-out/To-work-on rules in `ui/sampling.rs`, `scores::timeline_dots`, `StationAsk::of` (duplicates `ContestSettings::missing()`) and the QSB step math in `ui/band.rs` belong in cw-core.
- **Undo state outlives its toast.** `trash`, `before_restore` (which can hold the whole history), `before_reset` and `before_starter_words` are cleared only by their own Undo: when the toast goes without one, they stay until the next delete, restore, reset or word list replaces them. Each should be tied to the toast's lifetime.

## 3. Concurrency, lock-up and crash analysis

### 3.1 The threading model

App state lives on **the UI side** in Dioxus signals and `Rc<RefCell<…>>`, apart from the mixer the audio callback shares and the history files the persistence worker owns. That is one thread on the desktop and the web. On Android it is two threads that take turns under Dioxus's runtime lock, the webview's for taps and the event loop's for tasks, so a thread-local differs between them (the cause of `5110af3`). Async tasks interleave at each `.await`. Real parallelism is confined to a few threads, each with a narrow interface. The main ones are:

| Thread | Talks to the UI thread through | Failure handling |
|---|---|---|
| cpal audio callback (native) | `Arc<Mutex<Mixer>>` taken with `try_lock` only; sidetone edges in atomics; `failed`/heartbeat atomics | `catch_unwind` around rendering; a held lock plays one buffer without the band and the Morse (the sidetone, which needs no lock, still sounds) instead of waiting |
| Persistence worker (`persist/worker.rs`) | request/reply channels; replies resolve futures | each job runs under `catch_unwind`; a dropped reply becomes `None` rather than a hang; `flush` is bounded (3 s). The history read at launch is still waited for with no deadline (§5) |
| Backup reading, file picking | one-shot channels | a dropped sender reads as nothing read: "could not be read" for a file, "a Dust backup whose sessions cannot be read" for a backup |
| Sync HTTP (`ureq`, native) | one thread per request, result over a channel | 120 s global timeout, 64 MiB response cap; a dead thread gives an error |
| Desktop OAuth loopback | stop flag + channel | non-blocking listener, 10-minute limit, per-connection timeout, request line capped, PKCE S256 + random `state` |

Beside these, the persistence worker reads and writes a long history on scoped threads of its own (up to 4 readers, up to 8 writers), the OAuth listener serves each connection on a thread of its own, Android runs two small helper threads (`dust-loop-kick`, `dust-stall-log`), and the desktop paddle sleeps on blocking tasks.

**What keeps the app running.** Three pieces exist to keep the app from stalling, and were read as part of the runtime review: `keep_running.js` gives a hidden webview window a timer in place of the animation frames WebKit stops sending it; on Android, `dust-loop-kick` wakes the event loop every 250 ms as a backstop to the lost-wake fix in the patched `vendor/tao` (out of scope here, checked by CI); and `stall_log` reports a task more than 1.5 s late (one fix, `e603c57`). None was tested on a device here.

**What a panic does.** No panic hook is installed and the release profile keeps the default (unwind). On native builds Dioxus contains a panic during rendering and shows an error element; on wasm it does not, and a panic ends the app. What a panic in an event handler or a task does, where C-1 and S-6 panicked, was not checked; production code has no `unwrap()` and six `expect` calls, each on an invariant (Appendix A), so the remaining risk is mainly indexing, as in C-1.

What was checked and found **sound**:

- **Borrows.** Every `RefCell`/signal guard in the runtimes is scoped to a statement or block. None is held across an `.await`; clippy enforces this for `RefCell`, and it was verified by hand for generational-box guards. No guard is held while the same value is written. The one deliberate re-entrancy (the answer box handing an input to the machine during `dispatch`) is designed out by `closes_answer` never closing on input.
- **Stale work.** Session/run tasks check their `Lease` after each await, timers carry ids, the audio player carries an epoch/turn/ask triple, and floating windows carry generations. News from an ended or replaced session is dropped rather than applied.
- **Locks.** No lock is held across I/O. Poisoned mutexes are recovered (`into_inner`). The audio thread never blocks on the UI.
- **Persisted data.** All `unwrap()` and `expect()` calls under `src/persist` are in test code. Reads are lenient and bounded: serde_json's nesting limit everywhere; a backup's size, each session in it and its settings; `SENT_MAX`/`TABLE_MAX` in alignment; and now budgets in all for what a backup holds that this build does not read and for its run logs. `LONGEST_ANSWER` caps what is typed.
- **Numerics in the audio path.** Sample rates are floored at 1 Hz, every IIR/AGC entry is NaN-guarded, filters are stable at any rate, and subnormals are settled. No allocation or lock happens per sample.
- **Ordering of user actions.** Restores now run one at a time, each merged with the history the one before it left (`41eb9f1`), and an Undo takes its turn with them (`5208778`).

### 3.2 What could lock up or crash: found and fixed

| ID | Kind | Where | Trigger | Fix |
|---|---|---|---|---|
| C-1 | **crash** | `cw-core/src/contest/machine.rs` | the browser skips audio ahead of the ear (`TurnMissed`), then Esc: `index out of bounds: the len is 0 but the index is 0` | only true rests from an earlier turn are carried; callers the cut message brought go with it (`78ab7c6`) |
| AUD-1 | **UI hang (permanent)** | `src/audio/native.rs` | the audio device dies mid-write (USB DAC unplugged, sleep/resume, sound server gone); cpal 0.15's ALSA worker loops on `writei` errors without checking its stop pipe, and dropping the stream joins it on the UI thread | a broken stream is retired, not dropped, on ALSA hosts (`d4763dc`, `3d13aa0`) |
| N-2 | **crash** | `src/audio/native.rs` | cpal's ALSA worker dies by its own panic (unexpected timestamps, upstream cpal#630) without calling the error callback; dropping the stream then panics in cpal's `Drop` on the UI thread | a silent stream is retired too; any other close is contained with `catch_unwind` (`03a722f`) |
| AUD-D | **UI hang (permanent)**, a regression caught before release | `src/audio/native.rs` | the slow-start allowance (a stream gets 5 s to be called the first time) also reached the drop decision: a stream never called and let go 1.5–5 s after it opened was joined | letting go asks whether the worker has been silent for 1.5 s, whether or not it was ever called (`014f244`) |
| AUD-L | thread leak, a consequence of the AUD-1 fix caught before release | `src/audio/native.rs` | a stream stuck in an error loop was reopened at every paddle edge, and each stream retired since AUD-1's fix left its worker looping on its errors for good | reopened at most once a second whichever player asks (`e742371`, `49c1587`), and not for a second once the stream opened in its place loops too (`c3686fe`); a retired worker waits a second between errors (`9509678`) |
| S-6 | **crash** | `src/sync/ctx.rs` | sync turned off between a sync's spawn and its first poll (any sync, the Sync now button's included): `expect("sync is on")` | the sync re-checks its status; the expect became an early return (`7f9a9c5`) |
| P-8, PB-3 | **abort (out of memory)** | `src/persist/backup.rs` | a gigabyte file picked to restore (Android offers every file) was built up many times over: 16 bytes for every element of a list of tiny values (≈8 GB), one JSON value per element of the mark, or unknown keys kept per session | the list read one element at a time; numbers read in place; settings and sessions bounded; budgets in all for unknown keys and for run logs (`9c44e99`, `4a9b667`, `cc7518e`, `9109431`) |
| UIA-2, N-9 | **abort (out of memory)** | `src/ui/file_pick.rs` | a large non-UTF-8 file (a film picked by mistake) was copied lossily at 3–4× its size, or read whole as text on a phone | strict UTF-8 from the first slice; room made only once the first slice reads as text (where the browser has `Blob.arrayBuffer`; Safari before 14 still reads a file whole) (`e2fc65e`, `1009210`, `4f9fae7`) |
| SS-3 | crash (debug) / data loss | `cw-core/src/score.rs`, `stats.rs`, `alignment.rs`, `session.rs`; `src/persist/record.rs` | a stored count ≥ 2³¹ overflows; an infinite score is written as JSON `null` and the record never reads back | saturating arithmetic; a non-finite score is 0; a stored `null` score reads back (`a8e9be8`, `63b40fb`) |
| D-1 | latent crash | `cw-core/src/rng.rs` | `normal_within` with a negative or NaN limit panics in `f64::clamp` | inputs sanitised (`78ea397`) |
| C-2 | **UI freeze** | `cw-core/src/contest/bot.rs`, `caller.rs` | "Miss your exchange" at 100 %: Hear a QSO simulated an hour of failed turns synchronously on the UI thread | a caller misses at most once per QSO; the simulation is bounded per QSO (`fbeeac6`, `98f9a22`) |
| P-2 | **UI freeze** | `src/persist/files.rs` | the first launch over an older `sessions.json` fsynced once per session while the first window waited with no deadline: seconds for 3,000 sessions, most of a minute on a slow disk | the move stores records on up to eight threads at once (3,000 sessions: 2.1 s before, 0.4 s after, here) (`e64dce2`) |
| P-9 | **boot screen forever** (web) | `src/persist/web.rs`, `src/persist/idb.rs` | an IndexedDB transaction that never settled | the preload gives up once the database has kept it waiting 20 s; that visit keeps the history in page storage, and says so (`211c61f`) |
| CS-2 | UI stall | `cw-core/src/callsign.rs`, `words.rs` (the sampler in `stats.rs`) | Stats → Sampling rebuilt the prefix tables 6,000 times (≈0.65 s debug, ≈0.24 s release per open) | tables weighed once and kept (callsigns ≈15× faster); word scan made linear (words ≈4× faster) (`3f7c0ba`) |

## 4. Defects fixed

Each fix was made test-first where its code can run here: a regression test was written and seen to fail on the code before the fix, then the fix made it pass. The exceptions are named in their commits: paths only a browser or a phone takes (`211c61f`, `ee3e218`, and in later rounds `105ab0e`, `1009210`, `7670958`, and the browser halves of `60baa26` and `1c1b2ec`), a desktop fallback no test reaches (`5cde976`), a wait on the disk (part of `cb833bb`), a merge repair (`914c88e`), an abort that takes gigabytes to reach (`9c44e99`, whose new cases join a compatibility test), and a speed-up measured by a timing run by hand (`e64dce2`). Each fix set was reviewed independently. A reviewer's rejection led to a corrected follow-up, or to the fix being left out (one, §5.3), never to a waived test. Many of the fixes below were completed or corrected in later rounds; §4.4 lists those follow-ups. Severity: **Critical** = a crash, hang or data loss in ordinary use (none was); **High** = the same in a plausible edge case; **Medium** = the same only in a narrow race, or wrong behaviour most users would notice; **Low** = minor, latent (not reachable from the app as shipped), or reachable only with a crafted file.

**Terms.** The rows use the app's own words. A copy of the app is a tab or a window on the same store. To *take up* is to adopt what another copy or device changed; to *put back* is to restore a deleted session (a restore, or an Undo); to *let go* is to delete a session here because the drive deleted it. A *mark* is the file that says a session is under way, so that one cut off is kept at the next launch. A *history pack* bundles stored records on disk; a *sync pack* is a sealed file of changes in the drive. To *retire* an audio stream is to leave it open and silent rather than wait on its worker. A *lease* is a session's claim on the audio player; news from an ended lease is dropped.

### 4.1 Crashes, hangs and freezes

| Severity | Area | Defect | Commit |
|---|---|---|---|
| High | contest (web) | Esc after the browser skipped audio ahead of the ear indexed a station that Esc had just removed: `index out of bounds` (found by the extended soak, then independently by review) | `78ab7c6` |
| High | audio (Linux) | Dropping a stream whose device died (USB DAC unplugged, PCM suspended by sleep, sound server gone) joined cpal 0.15's ALSA worker, which loops on `writei` errors forever. The UI thread hung for good | `d4763dc`, `3d13aa0` |
| High | audio (Linux) | A cpal ALSA worker that died by its own panic (unexpected timestamps, upstream cpal#630) made cpal's `Drop` panic on the UI thread when the output slept or followed a new default device: the app crashed | `03a722f` |
| Medium | sync | A sync due 3 s after a change still started during "Turn off and delete the copy", leaving a pack or settings file in the drive with no keyring; and a sync spawned just as sync was turned off panicked on `expect("sync is on")` (a narrow race, on a computer or a phone) | `7f9a9c5` |
| Medium | contest | "Miss your exchange" at 100 %: pile-up QSOs could never complete, and Hear a QSO simulated an hour of failed turns synchronously on the UI thread | `fbeeac6` |
| Medium | persistence | First launch over an older `sessions.json`: one fsync per session while the first window waited with no deadline (2.1 s measured here for 3,000 sessions, most of a minute on a slow disk; 0.4 s after the fix) | `e64dce2` |
| High | web | An IndexedDB transaction that never settled left the boot screen up for good | `211c61f` |
| High | web | Restoring from a large non-UTF-8 file held its bytes and a lossy copy at once, 3–4× the file's size, and aborted the wasm app | `e2fc65e` |
| Low | persistence | A backup made of a huge list of tiny values allocated ~16 bytes per element (≈8 GB for 1 GB) and aborted | `9c44e99` |
| Low | sync | A tampered keyring could demand 1 GiB of memory and 64 Argon2 passes on the UI thread (now bounded to 256 MiB and 16, though still run on the UI thread) | `8fe6d88` |
| Low | sync | The web fetch had no timeout, and the Android sign-in poll had no deadline (a lost answer polled forever) | `ee3e218` |
| Low | cw-core | A send count of 2³¹, or tallies summing past `u32::MAX`, panicked in debug builds; a count past 2³¹ gave an infinite score, written as JSON `null`, and the record never read back | `a8e9be8` |
| Low | cw-core | `normal_within` panicked in `f64::clamp` for a negative or NaN limit; integer picks fell outside fractional ranges | `78ea397` |
| Low | scores UI | Two sessions with one timestamp were keyed alike: dioxus's debug assertion panicked, and release builds could diff the wrong row | `cabe66b` |
| Low | Stats | The Sampling tab rebuilt the call-prefix tables ~6,000 times per open (≈0.65 s debug / 0.24 s release at its worst row) | `3f7c0ba` |

### 4.2 Data loss, duplication and integrity

| Severity | Area | Defect | Commit |
|---|---|---|---|
| Medium | sync | Turning sync off while a sync ran was undone: the finishing task wrote the data key and refresh token back, a sync queued meanwhile ran at once while the card said Off, and the next launch synced again | `bd9f541` |
| High | sync | A session stored under two record names (backup restore vs. live) was taken by each device from the other and counted twice everywhere | `061ae21` |
| Medium | security | `sync.json` (data key and OAuth refresh token) was written world-readable (0644) on desktop Unix | `bef3ec4` |
| Medium | sync | A setting changed while a sync ran was reverted when that sync took a change from another device | `a7a3eb0` |
| Medium | auto level | A fall in a shorter alphabet overwrote the level a longer one had reached (20 → 3) | `0807d17` |
| Medium | auto level | Recovering a cut-off session after the level had moved applied an absolute level over the current one (8 → 6) | `7ee275e` |
| Medium | persistence | Desktop: another process's writes were marked as seen by this one's own restamp, so its changes did not show until the app was opened again | `8855056` |
| Medium | restore | A restore while the store could not be listed merged against an incomplete history and stored sessions twice | `7b85330` |
| Medium | history | A session another copy stored as it stopped was stored a second time under another id | `8fa83ae`, `914c88e` |
| Medium | desktop (macOS) | Cmd-Q/Quit only produces `LoopDestroyed`, so the running session was not ended and kept as on Exit, and pending settings were lost | `db5f6b1` |
| Medium | web | A session ended as the page closed was reported "cut off" at the next visit, because the "over" flag was never persisted | `fbecd68` |
| Low | persistence | A UTF-8 byte order mark made settings, legacy history and counters read as unreadable (settings reset, history moved aside) | `8a2835f` |
| Low | persistence | Counter entries a newer build wrote were dropped on the next save | `6f8fc61` |
| Low | persistence | A record packed by a newer build between listing steps vanished from the listing; one unreadable pack emptied the whole history listing | `bd44190` |
| Low | persistence | A directory fsync refused by the filesystem (vboxsf, network mounts) failed every save forever | `d60ec92` |
| Low | persistence | One session stamped far in the future pinned every later record's order at `u64::MAX` | `29b8766` |
| Low | sync | A pack written by a newer build was marked as read forever, so its records never arrived after updating | `2c41b89` |
| Low | sync | All put-backs went into the first pack with no size limit; after a large Undo every sync failed | `d514962` |
| Low | sync | A failed "Turn off and delete the copy", asked while a sync ran, left the card on "Syncing…" (Sync now greyed out) until a later sync ended | `fee1508` |
| Low | Android | The history continuation list was thread-local, but taps run on another thread: one entry leaked per action | `5110af3` |

### 4.3 Wrong behaviour users would notice

| Severity | Area | Defect | Commit |
|---|---|---|---|
| Medium | scoring | A trailing skip `_` scored as an error and a miss while accuracy said 100 % | `69e625d` |
| Medium | audio | The paddle sidetone reported success on an output that would not open, so the pad keyed silently | `cc4720f` |
| Medium | audio | Any transient ALSA error (EINTR on poll) failed the send in flight and rebuilt the band with a new seed | `3d13aa0` |
| Low | input | IME composition: the Enter that committed a composition confirmed the group or sent the run's message | `f0e9f24` |
| Low | input | Unbounded paste into the answer box or the Call and Exchange fields. A pasted page was keyed on the air | `23b51a6` |
| Low | settings | A level typed for one character set was committed to another after a mode switch | `ecbe18d` |
| Low | scores | A row pinned on one board left the timeline with no mark on another | `225af9d` |
| Low | sidetone | After ~101.8 days of the app running, every sidetone edge collapsed onto buffer boundaries | `90e21e9` |
| Low | mixer | A send that produced NaN muted the band too while it played | `58a5012` |
| Low | recognition | Inserting a missed first letter afterwards untimed the whole group and polluted copy-time medians | `2e7cea5` |
| Low | calendar | A date without zero padding counted for the streak but never appeared on the calendar | `31f6558` |
| Low | timing | NaN voice/fist/smoothing values planned NaN tones through the crate's public entry points (latent: the app clamps settings first) | `94c1924` |
| Low | Back button | After a reload away from Home, the first Back did nothing | `18581e1` |
| Low | Android | The stall logger tracked the last wait rather than the earliest due | `e603c57` |
| Low | performance | Every store write discarded every cached figure, and the screen shown worked its own out again; a results page re-read every run log on each redraw | `ef2d8a5`, `5870482` |

### 4.4 Rounds 2 to 5: review follow-ups and the rest of the confirmed findings

Where a row lists several commits, the later ones usually complete or correct the first, because a reviewer rejected it or found it incomplete; a few rows instead group related defects of one area (timing and scoring, performance, clock stamps).

**Regressions this review's own fixes introduced, each caught by a later round before release:**

- `fbeeac6` (a caller misses your exchange at most once per QSO) left a dupe unable ever to miss it in its new QSO: `98f9a22`.
- `7ee275e` (a cut-off session's level change is applied relative to the current level) compared the clamped level, dropping a rise earned in Koch once Mixed was chosen: `628f6aa`.
- `2c41b89` (a newer build's pack is no longer marked read) had such packs downloaded again at every sync and reported as damaged: `b089857`.
- `3d13aa0` (a transient ALSA error no longer fails the stream) left a dead device reported present to the paddle sidetone: `a58ea00`.
- `d4763dc` (a broken stream is retired rather than joined) left each retired worker looping on its errors for good, and `a58ea00` then retired a looping stream at every paddle edge, and each try of a failed send another: `e742371`, `9509678`, `49c1587`, `c3686fe`.
- `d76dfff` and `b7c93fb` (a stream gets five seconds to start, and a send waits for it) had a stream never called joined when let go (`014f244`), and a dead device hold each send five and a quarter seconds per try (`f4ea4af`).
- `9bc4e57` (a turn-off in another window is taken up) took any failed read of `sync.json` for a turn-off: `0a81487`.
- `963070c` (a newer build's timing values are kept) could write a timing's key twice, making the session unreadable: `e5f3b3d`.
- `105ab0e` (a browser reads a file through its own promise) decoded UTF-16 word lists as garbage and threw in Safari before 14: `60baa26`.
- `03aa69b` (the browser's history notes what it saw only on a listing) added a third read of the whole page-storage history at every change: `2a8e9c5`.
- `67c5c08` (a restore reads again a record here this build could not read) had a record that still could not be read refuse the first restore at every launch, and a later build's record leave out every session of its millisecond, whatever its day: `ad4fc69`.
- `6edc97d` (the sync card goes by the day the screen keeps) told a sync made just after a computer woke past midnight by its full date: `a35dc95`.
- `d3833c1` (a hand deletion sent is noted) kept that note over a putting back and a deletion made here between two syncs, so another device's putting back took the session back over this device's last deletion: `a2b19dc`.

**Crashes, hangs and freezes**

| Severity | Area | Defect | Commit |
|---|---|---|---|
| High | audio (Linux, BSD) | **Regression of the slow-start fix, caught before release:** a stream never called and let go between 1.5 s and 5 s after it opened (output asleep, a new default output) was joined, and could hang the UI thread for good | `014f244` |
| Medium | audio (Linux, BSD) | **Regression of the broken-stream and dead-device sidetone fixes, caught before release:** every stream retired since `d4763dc` left its worker looping on its errors (about fifty times a second) for good, and a stream stuck in a loop of errors was opened again at every paddle edge and at every try of a failed send on a player built afresh (some four streams per failed send), each leaving one more such worker. Now a stream is opened again at most once a second, whichever player asks, none for a second once the one opened in place of a looping one loops too, and a retired worker waits a second between errors | `e742371`, `9509678`, `49c1587`, `c3686fe` |
| Low | backups | A large JSON file picked by mistake, or a crafted one, was built up in memory many times over (a gigabyte list in the backup's mark about sixteen times; a gigabyte of keys no build reads, or of run logs, kept whole), and the app was stopped for want of memory. Android offers every file for a restore | `4a9b667`, `cc7518e`, `9109431` |
| High | Android, iOS | A film or photo picked by mistake was read whole as text by the phone's picker: up to a gigabyte, held three times over, enough to end the app | `4f9fae7` |
| Medium | web | Room for a whole large file was made before its first slice was checked, so a film picked by mistake grew the page by up to a gigabyte (completing round 1's `e2fc65e`) | `1009210` |
| Low | timing / scoring | A NaN pitch planned NaN tones; an error count at `u32::MAX` overflowed; the expected-error factor of a group of thousands of characters came to NaN or overflowed (all latent) | `27c02a0`, `12f8bbb`, `8b4419c` |
| Low | performance | The History tab was redrawn for every setting written; every store write re-keyed every session for the sampler's prior; a slider drag spawned a task and a timer per step; the page storage read the whole history three times per change (twice in 0.16.9; `03aa69b` added the third) | `f669a31`, `b2162b0`, `f4874a3`, `2a8e9c5` |

**Data loss, duplication and integrity**

| Severity | Area | Defect | Commit |
|---|---|---|---|
| High | sync | A session deleted by hand on one device and then put back on another (restored, or its deletion undone) was deleted again everywhere at the first device's next sync: a hand deletion was sent again whenever the drive said the session was there, whatever came after it. Hand deletions an earlier build made here, sent from here or found already in the drive, are noted once, at the first sync after updating | `d3833c1`, `b9df96d` |
| Medium | restore | An Undo in flight and a restore chosen meanwhile: sessions both held were taken for ones already here, then deleted by the Undo. An Undo clicked while a newer restore was under way undid the older one underneath it; and, after that fix, an Undo still offered once another restore was asked for did nothing, without a word | `5208778`, `1e14269` |
| Medium | contest | In a pile-up, a QSO lost unlogged (at End run, or when its caller gave up) could keep, as its copy, a call that was typed for another station and that station took. Found by the extended soak on a fresh seed range; present in the original code | `b2b09c1`, `6baa1cd` |
| Medium | history | **Regression of a round-2 fix, caught before release:** a timing's unread value and its typed value were both written under one key, so a session a restore filled in became unreadable in any build, and sync spread it | `e5f3b3d` |
| Medium | history | A session cut off was lost for good if the app stopped while the store answered which of its sessions were deleted, or while the store was looked at before they were kept | `96aaa4f`, `f99c8aa` |
| Medium | restore | A restore made while a listed record could not be read stored that session a second time. So did one beside a record this build could not read: a later build's showed the session twice, and one that had failed to read for a moment did so at the next launch. **After that fix, caught before release:** a record that could never be read (a damaged file) refused the first restore at every launch, and a later build's record left out every backup session finished in its millisecond, whatever its day | `6607123`, `67c5c08`, `ad4fc69` |
| Medium | restore | A second restore made while the first was being stored took the first's sessions for its own, and its Undo deleted them | `41eb9f1` |
| Medium | backups | Leaving the History tab while a backup file was read dropped the restore without a word. Two files chosen were both merged, each with its settings. A replaced file was still read to its end and parsed, and a read that never ended held off every later restore | `41687f6`, `ed1baa0`, `e9da3b4` |
| Medium | persistence | A record a later build packed while the history was read was told of as unreadable and not taken up while the app ran, and a backup written meanwhile fell back to the app's own copy, or left out (saying so) a record this build cannot read. A backup over a pack that would not open left out its sessions without a word | `5a30155`, `b92967b` |
| Medium | sync | **Regression of the cross-copy fix, caught before release:** any failed read of `sync.json` (a moment's Windows sharing violation) counted as "turned off elsewhere", and sync deleted its own key and sign-in | `0a81487` |
| Medium | sync | Turning sync off in one window was undone by another window's next sync on the same store. After that fix, a window taking up the turn-off cleared the store, deleting the state another window had just kept by turning sync on anew, or wrote its own over it, and the card said nothing of why sync was off | `9bc4e57`, `6b1c4c1` |
| Medium | sync | Delete versus put-back, and settings changes, were decided by raw wall-clock stamps from different devices, so a restore could be undone, a deletion fail to spread, or a setting changed on a device whose clock is behind be reverted. Stamps now follow the newest the drive holds (hybrid-logical). An Undo made while a sync ran could still tie with the deletion that sync sent, and was deleted everywhere | `fb27f52`, `75107c4`, `6665140` |
| Medium | sync | A session put back after sync was turned off and on anew, while a sync begun before that was still ending, was dropped with that stale sync's state, then deleted again | `3331c32` |
| Low | sync | A session let go by sync, put back by a restore and then deleted by hand came back at the next sync; so did one put back and deleted again before a sync, once another device put it back (for a deletion sent from here, a regression of `d3833c1`, caught before release) | `0e5bd7c`, `a2b19dc` |
| Low | sync | Turning sync off here, or a sync that ended the whole sync, cleared from the shared store the new sync state another window had just kept (made worse by `9bc4e57`, caught before release: since it, the other window takes the emptied store for a turn-off) | `87a6260` |
| Low | sync | A request under way as sync was turned off and on with another account could take the new sign-in's token on its retry, and write into that account's drive with the old key, or read a listing's later pages from it. On a computer, a token trade under way across a turn-off, or as another sign-in began, kept the first account's token for the new sync, or cleared the new refresh token | `4d1969e`, `0e2ab97` |
| Low | restore | Latent (the card never queues a restore, §5.4): a restore waiting its turn in the queue `41eb9f1` added was made inside the first one's callback, before the store's answer was taken up, so a session the store had let go for room was stored again | `1fdebe7` |
| Low | sync | A stale sync went on asking the drive after a turn-off, and could write files sealed with the old key into another account's drive | `ec5914e` |
| Low | sync | Sessions a sync stored before a later request failed stayed hidden until relaunch | `31da511` |
| Low | sync | After a failed "Turn off and delete the copy", nothing asked for a sync until the next change, or until the app came back into sight after two minutes | `30cc711` |
| Low | sync | **Regression of the newer-pack fix (`2c41b89`), caught before release:** a newer build's packs were re-downloaded every sync and reported as damaged | `b089857` |
| Low | persistence | A look that could not list the history left another copy's change marked as taken up, on a computer or a phone and in the browser: it never showed until the app was opened again (in a browser, until another change) | `cb833bb`, `03aa69b` |
| Low | persistence | A pack that could not be opened for a moment made the app store its records again beside it | `3313397` |
| Low | restore | Sessions a backup held that this version cannot read were dropped without a word, and a restore that brought none back said "Every session in it was already here" | `606a84f` |
| Low | history | Sessions an older build had written with `"score": null` could never be read again; they read and score again now (recovers lost data) | `63b40fb` |
| Low | history | A newer build's `untimed`/`charTimes` values under known keys were dropped on rewrite | `963070c` |
| Low | Android | A file chosen while a large one was read was spliced into it | `b4bfb12` |
| Low | Android | Sign-in answers that arrived after Rust gave up were kept for as long as the activity lived | `7670958` |
| Low | words | A word list saved with a byte order mark lost its first word on desktop | `fa69b97` |

**Wrong behaviour users would notice**

| Severity | Area | Defect | Commit |
|---|---|---|---|
| Medium | audio | A device slow to start (a Bluetooth headset or a USB card waking) failed every send with "Audio playback stalled" | `d76dfff`, `b7c93fb`, `cdc8903` |
| Medium | sidetone (Linux) | **Regression of the passing-error fix (`3d13aa0`), caught before release:** a dead ALSA device was reported present to the paddle, which keyed into silence | `a58ea00` |
| Medium | auto level | **Regression of a round-1 fix, caught before release:** it compared the *clamped* level, so a rise earned in Koch was dropped once Mixed was chosen; a change not applied was still announced | `628f6aa` |
| Low | audio | **Regression of the slow-start fix, caught before release:** a dead device held each send five and a quarter seconds per try, against 1.75 s before | `f4ea4af` |
| Low | sync | A last-sync stamp in the future (clock set back) stopped syncing on coming back into sight | `9adb8f8` |
| Low | contest | **Regression of a round-1 fix, caught before release:** a dupe that missed your exchange in its first QSO could never miss it in its new one | `98f9a22` |
| Low | sync | Cancel pressed as a sign-in finished was undone by the passphrase prompt. A sign-in under way as sync was turned off was kept. On Android, a second tap on "Sign in to sync" or on the corner's cloud then dropped the sign-in the user had just consented to | `8623601`, `c741b0d`, `b22f265` |
| Low | input (WebKit) | The Enter or Space that ends an input-method composition on macOS, iOS and Safari confirmed the group, skipped a character or keyed a run's message mid-word; the run's page keys sent TU and logged the QSO | `e8715da`, `1c1b2ec`, `18a766d` |
| Low | web | A file the browser could not read showed "Reading…" for good. After that fix, UTF-16 word lists loaded as garbage and Safari before 14 threw | `105ab0e`, `60baa26` |
| Low | Home, Scores, sync card | Left open past midnight, Home took yesterday's practice for today's, the Scores tab told a best set yesterday as set today, and the sync card told an evening's sync as made today. **After the sync card's fix, caught before release:** a sync made just after a computer woke past midnight was told by its full date | `831a165`, `3bb642c`, `6edc97d`, `a35dc95` |
| Low | word lists | An older word list, read after a newer one, was loaded over it | `9bf5b85` |
| Low | word lists | A word list saved as UTF-16 (Notepad's "Unicode") was refused on a computer and a phone, though a browser read it | `6bc3baa`, `5cde976` |
| Low | results | A session just finished was placed on the board by its timestamp, so another session finished at the same moment lent it its place | `440d11c` |
| Low | scores | Two runs with one timestamp showed one place on the board | `cbeea85` |
| Low | scores | A row pointed at that its board did not hold took the tapped row's mark with it | `823f8cd` |
| Low | auto level | Home counted towards a fall, or a rise, that had nowhere to go | `8ac47e9`, `aac4b55` |
| Low | sidetone | The edge clock's one-second look-ahead was relied on but never checked (a debug assertion now says so) | `79c9fdb` |

Four commits cited nowhere above change no behaviour (nor do `cdc8903`, a comment, and `6baa1cd`, a test and doc comments pinning `b2b09c1`'s fallback, which are listed with the fixes they go with): a doc comment moved back above its function (`ee061d5`); the test store aligned with the real stores' rule for noting changes (`078adbe`); a test that tries every key a group or a timing reads leniently for a key written twice, the class of the regression `e5f3b3d` fixed (`b65c0cc`); and the rule a later build must keep to for an older one to keep what it adds to a session, stated where the kept keys are (`f4ad2c4`).

## 5. What is not fixed, and why

These items are real, but each needs a design change rather than a local fix, depends on platform APIs that cannot be built or tested here, or is too minor to justify the risk of a change. Each item in §5.1–§5.3 has a recommended approach; §5.4's are noted only.

### 5.1 Needs a design change

| Area | Issue | Why it is deferred | Recommended approach |
|---|---|---|---|
| sync | **Several copies of the app on one device write their whole `SyncState`** (what `9bc4e57`, `6b1c4c1` and `87a6260` left). Each copy starts its sync from its own copy of the state, so another copy's notes (`applied`: deletions the drive holds; `revived`: put-backs not yet sent; the settings' `stamps`) can be overwritten, and are lost if that copy closes before its next sync. A copy that turns sync off or on anew elsewhere is now taken up and told of; the state of two live copies is not merged. | It needs a cross-copy lock, and a merge that respects removals. | Keep no long-lived copy of the state. Under the lock, re-read `load_sync()` at the start of each sync and of each note made outside one (`sync_revived`, a sign-in), apply only this copy's change to what was read, and write it back before letting the lock go (`take_up_sync_off` already re-reads before each write, but compares only device and keyring). Where the lock is not held across a sync's requests, merge three-way against the state the sync began from: `read` and the drive's `puts` take the union, `tombs` and `revives` the latest stamp, but `applied` and `revived` take only this copy's own additions and removals, since a sync removes entries on purpose. Serialise syncs with `navigator.locks` on the web and an advisory lock file beside `sync.json` elsewhere, and react to `storage` events on the web. |
| persistence | The marks of sessions cut off are deleted as they are read. Since `96aaa4f` and `f99c8aa`, this copy's mark carries those sessions as soon as the marks are taken, so only the moment between taking the marks and writing this copy's mark is left (in a browser, until the mark's Web Lock is granted). | A full guarantee needs a two-phase take in every store. | `take_cut_off` returns the marks still held; a new `Store::release_cut_off(ids)` is called once this copy's mark holds what they carry. |
| persistence | A record that failed to read only for a moment as the app opened is not read again while the app runs: `files.rs` `load` puts both kinds of failure in `unread`, and a look skips them (`state.rs` `sync_with_store`). A restore reads again those named for when one of its backup's sessions not here finished (`67c5c08`). | `HistoryLoad.unread` mixes "could not be read" with "this build cannot read it"; retrying all of them would re-read every later build's record at each look. | Keep read failures apart in `HistoryLoad` (`failed`), and have the next look read those again. |
| persistence | The legacy import's take-up cutoff and deletion watermark use the wall clock (`legacy.rs` ~515–547). | The cutoff is what stops an older copy's stale write-back from reviving sessions it let go before the move, and sessions let go before the move have no recorded identity. | Record the deletions taken (key and time) in the layout, and make every deletion not yet recorded. For sessions, keep a time cutoff, taking the newest stamp the move took where it is not ahead of the clock at the move (one session stamped far ahead, the case `29b8766` handled, must not stop every later take-up). |
| persistence | The first window waits on `Loading::wait` with no deadline while the history is read or moved (the move itself is now about five times faster, `e64dce2`). | Opening before the history is read is a product decision (what Home shows meanwhile). | A boot screen with progress past ~1 s, or open on an empty history marked loading. |
| audio (Linux, BSD) | Retiring a stream instead of dropping it (the workaround for cpal 0.15's ALSA worker, §3.2) has costs: the retired stream keeps its device open until the app exits, so on an exclusive `hw:` device every reopen fails until restart; a retired stream whose worker recovers keeps calling back about 190 times a second, playing silence; a single passing error (EINTR) at the moment the output sleeps or follows a new default can retire a healthy stream; and retired workers add up over a long session with a device that keeps failing (each now wakes once a second). | Ending a cpal stream needs its `Drop`, and on ALSA that drop joins a worker that may never stop: wherever it runs, that thread is held for good. | A dedicated audio thread that owns the streams (§6, phase 1) takes opening and dropping off the UI thread, but a stream that may be stuck must still not be dropped there, or that thread, and all audio after it, is held for good. For those, on Linux and the BSDs, a wrapper (an `unsafe impl Send`, sound for cpal 0.15's ALSA stream, whose parts are all `Send`) that drops retired streams on a throwaway helper thread under `catch_unwind`: it ends a worker looping on poll errors and closes its device; one stuck in its write loop holds only the helper. The workaround can go only with a cpal whose ALSA worker checks its stop pipe in the write loop, if one does. |
| Android, iOS | The phone picker's receive loop belongs to the file button (`file_pick.rs` ~538, `FileButton`): leaving the History tab while a phone still reads a large backup drops it without a word. | The callback the loop calls belongs to the card's scope (calling it after the tab is left panics), and the phone path cannot be run here. | Run each picker's loop once for the app's life in a root task, registered by id, and hand finished files to a callback created in the root scope and held by the restore under way. |
| persistence (forward compatibility) | Keys a later build adds inside a timing's `"charTimes"` or inside an entry of `"letterAccuracy"` are dropped the first time this build writes the session back (keys of the session, a group or a timing are kept). | Keeping them would cost every read of the history about a quarter to a half more (measured in this review, `f4ad2c4`), for keys no build writes yet. The rule for later builds (add beside `"charTimes"`, never inside it) is now stated on `UnreadKeys` (`f4ad2c4`). | Should a later build need keys there: hand-written one-pass readers for `CharTimes` and for a `letterAccuracy` entry that keep unknown keys without buffering, their keys checked against the typed readers' by a test. |
| settings (forward compatibility) | A value this build cannot read under a key it knows (a later build's new choice of an enum, say) reads as this build's default, and the next save writes that default over it, on this device and, through sync, on others. Unknown keys are carried; unknown values are not. | Carrying a raw value means telling "unread, keep" from "changed here, write" for every setting. | Keep the raw value of each key that failed to read beside the settings, and write it back unless that setting was changed here since. |
| test suite | The soak's coverage floors ("≥ 100 busted QSOs across the seeds") are tuned to the default seed range. In the final soak they failed on 3 of 7 ranges ("only 86 busted QSOs", "only 9 fumbled serials", "only 4 early exchanges", all under single callers), while every per-seed invariant held, pile-ups included. | Test-only. | Scale the floors with the number of seeds, or state them as rates with a tolerance. |

### 5.2 Platform behaviour that could not be checked here

| Area | Issue | Recommended approach |
|---|---|---|
| sync (Android) | A token Drive refused can be handed back from Play services' cache until it expires (`fresh` is ignored), and `PlayAuth` keeps no token to clear. | Have `PlayAuth` keep the last token it handed out, and on `token(fresh = true)` clear it first (`GoogleAuthUtil.clearToken`, or `AuthorizationClient.clearToken` where the pinned library has it) through a new JNI method. |
| persistence (Apple) | `File::sync_all` is `F_FULLFSYNC`, which some SMB/NFS volumes refuse, so every write there would fail (suspected, not reproduced). A refused folder flush (`EINVAL`, `ENOTSUP`) is tolerated on every Unix, Apple's included (`d60ec92`); a refused file flush is not. | Fall back to `fsync` when `F_FULLFSYNC` returns `ENOTSUP`/`EINVAL`. |
| audio | A device that dies without ever reporting an error (a hung sound server, an iOS interruption at stream start) now takes about 16 s (3 tries × ~5.3 s) to be reported stalled, against ~5 s before the slow-start fix; the sidetone takes it for present for up to 5 s per fresh stream. | The cost of not failing slow Bluetooth devices. A per-device memory of "started slowly before" would let a known-fast device fail fast. |

### 5.3 Behaviour and design decisions

| Area | Issue | Recommended approach |
|---|---|---|
| DSP | The AGC hang (220 ms at Fast decay, 600 ms at Slow) is shorter than a letter gap below about 16 WPM at Fast (6 WPM at Slow), and at higher effective speeds with Farnsworth spacing, so the hiss swells between letters: a few dB at 14–15 WPM, about 10 dB by 12 WPM. The docs promise no pumping. | Either fix the docs, or size the hang from the longest letter gap the settings give, Farnsworth spacing included, kept under the word gap. |
| restore | A restore leaves out a backup's session only where a record here that this build cannot read says, as JSON, that it holds a session finished at that moment on that day (`67c5c08`, `ad4fc69`). A record damaged so that it still says both is taken at its word, and a record a clock set back named after the newest is not matched at all (the backup's copy is then stored beside it: twice at worst). | Accept; or record, as each build stores a record, a format version the older build can read (§6, phase 1). |
| paddle (input methods) | A `[` or `]` typed while an input method composes still keys a paddle, and the paddle's handling keeps it from the input method (paddle keys are read by their place on the keyboard). Passing by every press marked as the input method's (`isComposing`, key code 229) was tried and rejected in review: on French, Belgian, Spanish, Portuguese, Swiss and Nordic layouts the paddle key is a dead key, which Safari, Chrome on a Mac and the macOS build report the same way, so those paddles would key nothing in the answer box or a run field. On Linux, tao reports no composition state at all. | Pass by a press only within a composition a paddle key did not open: remember the dead paddle key on keydown, clear it on `compositionend` or another key, and apply the same rule in the page script, the web listener and the root, with tests by place for a dead key with code 229. |
| security (web) | In a browser the sync key and refresh token live in `localStorage` under the GitHub Pages origin, readable by any page published there (documented in `docs/privacy.html`), and `index.html` sets no Content-Security-Policy. On Windows the permissions of `sync.json` were not checked. | Accept and keep documented, or move the web build to an origin of its own; add a CSP; restrict `sync.json`'s ACL on Windows. |
| sync | Narrow orderings: a put-back noted while a sync of a sync since turned off still runs keeps a stamp from the new, empty drive (what remains after `3331c32` and `75107c4`); a put-back made during a sync before that sync lists the store is let go by it (as in 0.16.9); a put-back noted in a copy that takes up a turn-off made elsewhere is dropped (the trade `9bc4e57` and `6b1c4c1` made for the turn-off to hold). | Re-stamp the notes a sync takes in once it has read the drive (after `read_packs` in `sync_once`), as notes made during a sync already are at its end. Have a sync let go of nothing put back since it began (look at the notes made meanwhile before its `change_history`). Keep `revived` across a turn-off taken up from elsewhere. |

### 5.4 Smaller items noted by the reviews

All Low; each was judged not worth the risk of a change in this review.

- **Restore and Undo.** The one-at-a-time rule holds within one tab or window: a restore in another tab that shares sessions with this tab's restore takes them for ones already here, and this tab's Undo then deletes them (a lock around a restore and its Undo, across copies, would close it). An Undo of a restore is not held back while a session runs, as the card's restore is, so it can put the old settings, level included, back mid-session. The card goes on saying "Reading …" through an Undo clicked within 16 ms of a restore ending.
- **Sync.** `Store::forgotten` cannot say that it could not look, so the one-time pass for an earlier build's deletions (`b9df96d`) can be spent without looking; a downgrade that drops its flag runs it again, and it then notes twin deletions too. A hand deletion is stamped when it is sent, not when it was made, and one also made elsewhere is noted with the drive's deletion time (`deleted_too`), so a putting back made earlier by the clock but synced later can win. A pack written whose answer was lost leaves its hand deletions un-noted until the next sync reads it back; should a putting back elsewhere reach the drive first, that sync sends the deletion again, deleting the session everywhere.
- **Audio.** A refusal for a looping stream outlives a stream that recovers by itself, for up to a second. Two transient errors in a row with no buffer between them now reopen the stream for a send or a slider move, as they did for the sidetone. Should a later cpal report passing errors off the audio thread (0.15's CoreAudio host reports only a disconnection there, after pausing the stream), one could interleave with a callback and leave a healthy stream taken for looping until its next callback.
- **Persistence.** Every listing holds the text of every packed record. A backup reads the packs again once per part of 256 records with a record in neither place. The page storage still reads the whole history twice to load or list it, and copies it to hash it. A crafted backup of sessions holding only bare timings or groups still reads into several gigabytes: the typed parts are bounded per session and per file, not in all (leaving out a session whose timings outnumber its groups would halve it; a budget in all would close it). A restore that fills in a run's lost log takes the backup's copy whole, which no existing build can lose anything by, as every build that drops a log drops every key it does not read. A record another copy deletes between the tombstones' listing and its own read is told of as unreadable. `live_keys` leaves out records that cannot be read now, so the older-build import could take one of their sessions up a second time.
- **UI.** A record renamed by a take-up while its Results page is up loses its place there. A browser reads a backup of up to 8 MiB saved as UTF-16 by its byte order mark, where a computer and a phone refuse UTF-16 (a UTF-8 mark is read everywhere). `board_place`, which finds a session by its timestamp, is now used only by cw-core's tests.
- **Tests.** The restore queue's queued path, and the branch of a failed Undo while a restore waits, are reached only in tests, as the card never queues a restore.

## 6. Refactoring plan

Ordered by risk removed per unit of effort. Phases 1–2 are worth doing before new features; phase 3 is hygiene to do as files are touched.

### Phase 0: before this branch is merged

Run CI's jobs that could not run here: the Android and iOS type-checks, desktop lint on Windows and macOS, and the web site's build with its browser checks. Then check by hand the fixes tested only against fakes:

- **Linux audio:** unplug a USB sound card mid-send, and suspend and resume, with Pulse/PipeWire and on a bare `hw:` device: no hang, the next device opens, a send fails within a few seconds (`d4763dc`, `03a722f`, `014f244`, `e742371`, `c3686fe`).
- **macOS:** Cmd-Q during a session keeps the session and the settings (`db5f6b1`).
- **Input methods** on macOS, iOS and Safari: a Pinyin or Kana composition ended with Enter or Space confirms nothing and keys nothing (`e8715da`, `1c1b2ec`, `18a766d`).
- **Browsers,** Safari 13 included: a backup and a UTF-16 word list load; a film picked by mistake is refused at once (`211c61f`, `105ab0e`, `1009210`, `60baa26`).
- **Phones:** the file picker with a large backup, a second file chosen while one is read, a UTF-16 word list (`4f9fae7`, `b4bfb12`, `6bc3baa`); Android sign-in with two taps on "Sign in to sync" (`ee3e218`, `7670958`, `b22f265`).
- **Windows:** a `sync.json` held open by another program for a moment does not turn sync off (`0a81487`).

### Phase 1: close the largest remaining correctness gaps (1–2 weeks)

1. **Cross-copy sync state.** Re-read `SyncState` under a Web Lock or a lock file before each change, and apply only this copy's change (design in §5.1). Two copies of the app on one device are where most of the remaining sync risks live: each starts its sync from its own copy of the state, so the notes one copy keeps (deletions sent, sessions put back) can be overwritten by the other's.
2. **A dedicated audio-control thread on desktop** that owns the cpal streams (`cpal::Stream` is not `Send`, so it must be made and dropped there), so opening, probing and dropping never run on the UI thread. A drop that waits on a stuck worker would then hold that thread, not the UI, but for good, and every open and drop after it. So the UI must never wait on that thread without a deadline, and a stream that may be stuck is still not dropped there: it is retired, or handed to a throwaway thread (§5.1). The retire workaround, with its costs, can go only with a cpal whose ALSA worker checks its stop pipe in the write loop, if one does.
3. **Typed errors at the boundaries.** Replace `Result<_, String>` in `MorseBackend`, `Http`, `Store::write_settings` and `HistorySaved::Failed(String)`, and the string catch-alls of `RemoteError` (`Failed(String)`) and `AudioError` (`Device(String)`), with small enums (`Transient`/`Permanent`/`Full`/`Newer`/…) implementing `Display`. That lets sync retry 5xx/429/timeouts with backoff; today a failed sync simply waits for the next trigger.
4. **A format version in every stored session**, written by each build. An older build could then tell a later build's record from a damaged one by what it says of itself, rather than by the restore's heuristic (§5.3), and tell, as the app opens, a record that failed to read for a moment (to read again, §5.1) from a later build's.

5. **A sync planner soaked like the contest simulator.** Sync's plan is already pure (`sync/plan.rs`); drive it with seeded random orders of deletions, put-backs, Undo, turn-offs and lost answers across several simulated devices and copies, and check that every device converges on what the last action meant. Most of the 47 sync and persistence entries in §4 are orderings such a soak finds.

The other §5.1 items (cut-off marks released only once held, read failures read again, the legacy import's watermark, unknown setting values carried, the phone picker's loop in a root task) and §5.3's sync orderings are local; each has its design in §5.

### Phase 2: structure (2–4 weeks, mechanical but broad)

6. **Split `ContestMachine`** (`contest/machine.rs`, ~5k production lines, 101 methods; 99 in 0.16.9) into `machine/{mod, air, plan, heard, log, undo, view}.rs`. Give stations a stable `StationId` instead of a `Vec` index that Esc truncates: that is the root of the crash fixed in C-1. Replace `Air`'s parallel vectors with one `Vec<PlannedSegment>`.
7. **Split `AppCtx`** into `HistoryCtx` (sync with store, delete/undo), `RestoreCtx` (restore and its Undo), `SettingsCtx` (store/merge, card undo) and `MarksCtx`, as in §2.2 (`Toasts` is separate already), each with its own signals. Have state return typed outcomes and let `ui::` word them. Together with item 8 and a neutral `text` module for the wording, that removes most of the module cycles listed in §2.4; `contest_runtime↔ui` also needs `ui::contest::field_name` and `ui::widgets::focus_end_script` moved out of `ui`, and `app↔page_closed` needs `app::store_settings_now` moved.
8. **One run runtime.** A `RunMachine` trait (start, apply, effects, view) and a generic `LiveRun<M>` replace the parallel skeletons of `session_runtime.rs` and `contest_runtime.rs`, with one heartbeat owned by the runtime (runs currently depend on a mounted component's `use_ticks`).
9. **One record-store core.** Make the algorithm duplicated between `persist/files.rs` and `persist/idb.rs` (10 function pairs) generic over a small backend trait; idb's `Glue` is close to it already. Split `Store` (37 methods; 36 in 0.16.9) into focused traits.
10. **Phase-specific state in enums.** `SessionMachine`'s `resume`/`countdown_to`/`missed`/… become fields of a machine-private phase enum mirroring the public `SessionPhase` (`Paused { … }`, `Playing { … }`, `Countdown { … }`). `WaitFlags` becomes an enum. `Caller`'s ~13 booleans split into `OperatorTraits` and a progress enum.

11. **A `Page` trait with a fake** for what the UI asks of the page (focus, scripts, file reading), as `MorseBackend` is for audio, in place of the `#[cfg(test)]` branches inside production functions, so tests run the path that ships.
12. **A storage-and-sync crate.** Sync's logic and the core of persistence need only cw-core and a clock; as a crate of their own the compiler would enforce the layering of §2.4.
13. **One way to serialise restores.** Keep the card's wait or `AppCtx`'s queue, not both (§2.3).

### Phase 3: hygiene (ongoing)

14. Move inline test modules (about half of most large files) to sibling `tests.rs` files, as `band/` and `persist/idb/` already do.
15. Extract the shared helpers that exist in 2–7 copies: `format::percent*`, `use_page_listener`, `focus_by_id`, `Lender<T>`, `DelayLine`, `TwoWindow`, one sidetone-constants module, `SessionResult::group_speed`, one `QsoRecord` constructor, and a `GridModel` trait for the span/speed beliefs.
16. Move domain logic out of the UI (forecast standing, the sampling "Finding out/To work on" rules, timeline dots, `StationAsk::of`, QSB step math) into cw-core, where it can be unit-tested without rendering.
17. Tie undo state (`trash`, `before_restore`, `before_reset`, `before_starter_words`) to the lifetime of its toast, so a restore's Undo does not keep a whole history alive for the rest of the run.
18. Make the soak's coverage floors relative (§5), let an environment variable move their seed range, and add an allocation-counting test around `BandMixer::next` in cw-core (the app's mixer already has one).

## Appendix A: examined and found sound

These looked risky and were checked in detail. They are recorded so they need not be checked again.

**Panics on data**
- All 240 `unwrap()` and 141 `expect()` calls under `src/persist` are in test code (test modules, the test-only `memory` store, and `idb/tests.rs` / `idb/page_tests.rs`); production persistence code has none, and no `panic!` or `unreachable!`. Stored JSON is read leniently, under serde_json's default nesting limit of 128. A backup's parts are size-capped (1 GB file, 4 MB per session, 1 MB of settings, 16 MB of keys this build does not read, 16 MB of run logs), and the alignment of a stored answer is capped (256 sent characters, 4 M table cells). Records in the store are read whole.
- User-typed calls are normalised to ASCII; exchanges are kept as typed (upper-cased) and sliced only at char boundaries. Numeric parsing is capped at 9 digits. `RecordId::parse` slices only after checking length and an ASCII `-`. `text::unpercent` and `unhex` cannot slice off a char boundary.
- Float sorts use `total_cmp`, but for two in `cw-core/src/stats.rs` that use `partial_cmp(..).unwrap_or(Equal)` on values that cannot be NaN; none uses `partial_cmp().unwrap()`. Every `f64::clamp` has ordered, non-NaN bounds: most are constants, and the runtime ones are guarded (`finite_within`'s `high.max(low)`, `low.min(high)` in `speed.rs`, the minimum clamped before the maximum in `clamp.rs`, and `normal_within` since `78ea397`). `clamp.rs` replaces non-finite settings before range-clamping, and is idempotent.
- Weighted picks tolerate empty, NaN and infinite weights. Rejection loops end quickly: callsign redraws are capped at 8, and the serial model's uncapped redraw rejects about 2 % of draws (WPX). `usize_in` tolerates reversed ranges.

- Outside `src/persist`, production code has no `unwrap()` and six `expect` calls, each on an invariant: building the GTK menu (`menus.rs`, two), `ui/endurance.rs`, `audio/stream.rs`, keying HMAC (`sync/seal.rs`) and a contest's row of rules (`cw-core/src/contest/mod.rs`).

**Concurrency and lock-ups**
- No `RefCell` or generational-box guard is held across an `.await`, and none is held while the same value is written. This was checked by hand for signals, beyond what clippy's `await_holding_refcell_ref` covers.
- Every runtime task acts only while its `Lease` holds: news from an ended or replaced session is dropped by the lease check in `dispatch`, and timer ids let a machine ignore a timer it has since replaced. A heartbeat is one per claim and ends within one beat (250 ms) of its lease. A timer task sleeps out its time (a run's end timer for up to an hour, one more for each Resume) and then does nothing.
- Every store's answer re-registers its waker on each poll (tokio's oneshot on the worker thread, the browser's promises, the test store's poll functions), so `settle_or_spawn`'s first poll with a no-op waker loses no wake.
- The persistence worker runs each job under `catch_unwind`, and a dropped reply resolves to `None` rather than hanging. `flush` is bounded to 3 s, no lock is held across I/O, and poisoned mutexes are recovered.
- The audio data callback never waits (`try_lock`), allocates nothing per buffer, and contains a panic in the mixer with `catch_unwind` (the sidetone and limiter run outside it; the error callback backs off on repeated errors by design). The memo `Mutex` in `band/memo.rs` is never locked from the callback.
- Sync HTTP (native) has a 120 s global timeout and a 64 MiB response cap. Drive requests are retried once with a fresh token after a 401 or a 403 sign-in refusal, and (since round 4) never with a token a later sign-in fetched. The OAuth loopback listener is non-blocking, time-limited (10 min), caps the request line, uses PKCE (S256) and checks a random 128-bit `state`. The Drive listing stops on an empty page token or one equal to the previous one.

**Cryptography (sync)**
- XChaCha20-Poly1305 with random 192-bit nonces and the file name as associated data. Separate sealing and naming keys are derived with HMAC labels. Argon2id parameters are stored in the keyring and bounded on read: since `8fe6d88`, to at most 256 MiB, 16 passes and 16 lanes (1 GiB and 64 passes before).
- Opening fails closed without panicking on a wrong passphrase, a truncated or tampered file, or empty input. `DataKey`'s `Debug` hides the key.
- Packs are authenticated before they are inflated, and inflation is capped at 64 MB.

**Numerics in the signal chain**
- Sample rates are floored at 1 Hz. The receiver filter is stable at any rate (|pole| = e^{Re(p)T} < 1) and is periodically settled out of subnormals. The AGC's look-ahead is ≥ 1 sample, so modulo-by-zero is impossible. Corners are prewarped to ≤ 0.45·fs, and every tone stays below Nyquist at 8 kHz.
- Farnsworth timing is continuous at effective = character speed, and an effective speed above the character speed is clamped down. The PARIS identity (31·dot + 19·unit = 60/eff) was checked by hand.

**Platform glue**
- `linux_webview::prepare` mutates the environment as the first statement of `launch_desktop`, the first thing `main` runs on the desktop, before `persist::preload` starts any thread, so its `unsafe` safety claim holds. The GTK menu's `unsafe` builds a `GdkEventButton` from `gdk_event_new(BUTTON_PRESS)`, with its window reference released by `gdk_event_free`.
- Paddles track contacts by physical key code, and every focus-loss path (blur, `Focused(false)`, page hidden, Meta key-up) releases all of them. No stuck sidetone was found.


## Appendix B: coverage by module

Every module was read by the ten bug-class sweeps (§1, step 3), which ran over the whole code base. Each was also given to one of the ten subsystem reviews, as below. "Fixes" points to §4.

| Area | Modules | Subsystem review | Outcome |
|---|---|---|---|
| Contest | `cw-core/src/contest/**` (machine, caller, bot, rules, esm, record, exchange, score, serial, turn, qrm, odds, parse, place, check, clock, data) | contest | Fixes (C-1, C-2, the lost-copy invariant); design in §2.2 |
| Signal chain | `cw-core/src/band/**`, `keyer/`, `timing/`, `morse` | DSP/keyer/timing | Fixes (NaN guards, `normal_within`); AGC hang in §5.3; design in §2.2 |
| Sessions and settings | `cw-core` `machine`, `session`, `settings/**`, `auto_level`, `level`, `endurance`, `aim` | sessions/settings | Fixes (levels, overflowing counts, lenient keys); unknown setting values in §5.1 |
| Sampling and analytics | `cw-core` `sampling`, `callsign`, `words`, `stats`, `span`, `speed`, `alignment`, `score`, `board`, `heatmap`, `streak`, `recognition`, `belief`, `buckets`, `pool`, `sequences`, `numeric`, `rng` | sampling/analytics | Fixes (skip scoring, calendar, timing medians, sampler speed, word-list byte order mark); duplication in §2.3 |
| Test support in cw-core | `soak`, `learner_sim` | — | Run, not reviewed as code; floors in §5.1 |
| Audio | `src/audio/**` (native, web, mixer, stream, wait, shape, tone, epoch, silent), `player`, `playback` | audio | Fixes (hangs, crash, slow start, sidetone, leaks); retire workaround in §5.1 |
| Persistence | `src/persist/**` (files, desktop, web, idb + `idb.js`, legacy, backup, record, worker, android), `history` | persistence | Fixes (packs, marks, backups, BOM, fsync, order); residuals in §5 |
| Sync | `src/sync/**` (ctx, run, plan, pack, seal, state, settings, drive, remote, http, auth, text, config) | sync | Fixes (turn-off, stamps, deletions, cross-copy, sign-in); design in §2.4, §5.1 |
| App state and runtimes | `state`, `session_runtime`, `contest_runtime`, `app`, `routes`, `time`, `stall_log`, `keep_running.js`, `keep_awake`, `page_closed`, `page_hidden`, `system_back`, `main`, `menus`, `linux_webview`, `theme` | app state and runtimes | Fixes (marks, Quit on macOS, Back button, stall log, restores); design in §2.2 and §2.4 |
| UI: training, run and input | `ui/` training, contest, contest_keys, contest_results, paddle, listen, hear_qso, home, results, odds, envelope, letters, discard, endurance, today, notice, floating, window_menu, display | UI (interactive) | Fixes (IME, paste, midnight, file picking); paddle input methods in §5.3 |
| UI: stats, settings and history | `ui/` stats, stats_detail, scores, figures, chart, heatmap, confusion, bigram, callsign_reach, sampling, forecast, auto_level, settings, settings_charset, settings_station, backup, file_pick, sync, sync_pip, history, tour, help, tips, credits, privacy, widgets, format, band | UI (stats/settings) | Fixes (boards, figures, restores, word lists, sync card); splits in §2.2 |
| Test support in the app | `testing`, `timing_tests`, `fake_idb.mjs`, `fake_page.mjs`, `*_tests.rs` | — | Used and extended by the fixes; not reviewed as code |
| Android activity | `android/MainActivity.kt` | sync (sign-in) | Fixes (`ee3e218`, `7670958`), not compiled here |
