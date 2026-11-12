# Work in progress: the record store and the run lifecycle, after §8

Saved on 12 November 2026, to be resumed. **Delete this folder before a pull request is opened from this branch.**

## Where things stand

The branch `claude/admiring-rubin-6z9yr6` holds both pieces of work, merged and verified:
- the record store as one core over the files, a browser's database and the test store (merged at `58133f1`);
- the run lifecycle as one explicit state machine under a session of groups and a contest run (merged at `984d718`).

At `984d718`, with Rust 1.99.1 and `CI=1`, these all passed:
- **Format and lints:** `cargo fmt --check`; clippy `-D warnings` for desktop, web (wasm32) and silent; the Android lint (`tools/android-clippy.sh`).
- **Tests:** cw-core 1,412 and its doctest; desktop 1,836, with 12 ignored; silent 1,705, with 12 ignored.
- **Test names:** the merged suites hold main's (`ab0cbf7`) tests, plus what each branch added, less the 17 the store's branch took out (15 near copies of conformance cases, and 2 moved under their own names).
- **Walks:** 4,000 lifecycle walks (2,000 of each kind) pass every check, and give the same lines, one for one, as `6a424c6`'s build.
- **Doc links:** the same broken set as before (13 in the app, 4 in cw-core).
- **Module graphs:** no cycle in the app under any rule; in cw-core only the `settings` family, under the two stricter rules.
- **Windows:** the simulated Windows lint is clean.

`docs/code-review.md` on the branch is still the version from before this account was written. Its §7.7 describes the store, and nothing describes the lifecycle yet. The new account, a §9 that replaces §7.7, is drafted here and is not yet in the report.

## Still to do, in order

### 1. Review and keep the fix of a defect the fact-check found

**The defect.** When the history kept whole must be moved in again, the files and the test store read the tombstones only once (`record_store::here_of`, under `Order::MoveThenRead`). That read comes between listing the records and reading them. So:
- a deletion that another copy makes in between is undone;
- that copy's Undo, made in between, can leave the session stored twice.

A browser's move, and the files' take-up, already read the tombstones before and after (`2ec6e8e`, `0e222e3`, `de8dd8d`). The store work's first review round left this gap "for a fix commit before step 2.1", and that commit was never made.

**The fix.** It was written test first and is saved here as `patches/0001-*.patch`, from commit `7250373` on a local branch that was not pushed. By its own message it ran:
- fmt and the three clippy configurations;
- the persist tests, on desktop and silent;
- the whole desktop suite, 1,843 passed;
- the walks.

**Not yet done for it:**
- an adversarial review;
- the whole silent suite;
- the Android lint;
- the Windows probe;
- doc links and graphs;
- the walks compared line by line against `984d718`'s build.

On the files, an Undo takes two steps. So one whose own two steps span this copy's reads can still leave the session neither here nor deleted, and so stored twice. That window is the Undo's own, as at a take-up.

```bash
git am wip/dedup-followup/patches/0001-*.patch
```

### 2. Finish the report

The `report/` folder holds:
- `s9-draft.md`: the new §9;
- `summary9.md`: the Summary's new paragraph;
- `verify-template.md`: §9.7's verification;
- `apply.py`: writes `report/code-review.new.md` from the current `docs/code-review.md` and those three files.

`apply.py` also edits the Summary, §2.3, §6 (including a new Phase 0 item for checks by hand), §7.4 and §7.6, and it removes §7.7.

These are still to fill, each marked `⟦…⟧`:
- §9.5: the defect above and its fix.
- §9.4: the last rows and the totals of the code-size table. Recount at the new head with `tools/mycount.py`, which gave every figure already in the draft: `python3 tools/mycount.py <rev> [--per-file]`.
- The Summary: the count of fixes in the stores (23 once the fix is in, against 22 before) and the growth.
- §9.6: the last bullet, on what the fix leaves.

Then redo §9.7's verification at the final head (`tools/verify.sh`, `tools/post.sh`).

### 3. Fact-check the revised text again

This round is for regressions only: check each correction taken from `factcheck/findings.txt`, and each claim that is changed or new.

The first fact-check found 82 problems in the first draft, most of them claims wider than the commits show. The draft here takes every one of them, as judged, and the move fix is new.

### 4. Commit, clean up and ask

Commit the report, delete this folder, push, and ask the owner before opening a pull request. CI runs these only on a pull request:
- the iOS and Android type-checks;
- the desktop lint on Windows and macOS;
- the web site's build with its browser checks.

## For the owner to decide

These are also in the draft's §9.6.
1. **Answer time in the score.** An interrupted group's answer time stays in the score, with the pause left out. Taking it out would score those sessions afresh in this build only, so one session would rank differently in two builds.
2. **The mark at a pause.** A pause now writes the mark. A session or run the app stops under while paused is kept as End would have kept it: its interrupted group answered as it stood, or its open QSO lost. It is counted towards its level once.
3. **The lifecycle's size.** It holds 18.4% more code lines than at `ab0cbf7` (814 to 964), against the 8–10% accepted: 10.7 points for the consolidation, and 7.7 for the fixes made first. The plan's one lever, dropping `Stage::Ending`, would save about 4 lines.
4. **A run's copy keyed across a pause.** It keeps the times of the keys typed before the pause, in a turn that played out. The session's all-or-nothing rule for it is offered, not made.
5. **A run's away check.** A run is watched for time away only from its screen's first tick, so a freeze as it starts counts as run time.

## Tools

The scripts the checks used are in `tools/`. Their paths are absolute to the machine they ran on (`/home/user/dust`, `/home/user/wt/...`, a scratch folder), so adjust them before use.
- **`verify.sh`:** fmt, the cw-core, desktop and silent tests, and the three clippy configurations, for one checkout with its own target directory.
- **`android-clippy.sh` with `fakecc`:** CI's Android lint, with a stand-in C++ compiler.
- **`doclinks.sh` with `doclinks-base-app.txt` and `doclinks-base-core.txt`:** broken intra-doc links, compared as sets with the baselines.
- **`modgraph2.py` with `graphs.sh`:** module cycles, under §7.1's three rules.
- **`winprobe.sh` with `probe.py`:** the simulated Windows lint. It compiles out the Unix-only test items, then runs the desktop clippy over all targets.
- **`walkdiff.py`:** compares two folders of walk lines. Write each folder with `DUST_LIFECYCLE=<dir> DUST_LIFECYCLE_SEEDS=<n>`, running `cargo test lifecycle_walk -- --ignored`.
- **`mycount.py`:** production lines. It walks the module tree from each crate's root, evaluates `cfg`, and ends each item by its grammar.
- **`post.sh`:** the merged head's checks beyond `verify.sh`: graphs, doc links, the Windows probe, walks against a branch head's, test names against both heads' and main's, and the Android lint.
