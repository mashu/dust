On the merged head, `984d718`, with Rust 1.99.1 and `CI=1` (so that a missing Node fails the page tests rather than skipping them):
- **Format and lints:** `cargo fmt --check`, and clippy `-D warnings` for desktop, web (wasm32) and silent, are clean. So is the Android lint, run here as §7.5 describes.
- **Tests:** they all pass: cw-core 1,412 and its doctest; desktop 1,836, with 12 ignored; silent 1,705, with 12 ignored. The ignored ones are run by hand: the 8 walks of §9.3 that take a count of seeds, and the 4 of §8.4.
- **Test names:** the merged suites hold, name for name, every test `ab0cbf7` held, with these exceptions: they add the 113 the store's branch added and the 52 the lifecycle's added, and they leave out the 17 the store's branch took out. Of those 17, 15 were near copies, in the stores' own tests, of what conformance cases now hold (`5d03f77`, `c6df3c8`, `0661bd4`), and 2 are a browser's tests moved, under their names, into the core's own (`4834961`). That makes 1,848 in the desktop suite and 1,717 in the silent one, ignored tests included.
- **The walks:** seeds 1 to 2,000 of each kind pass every check on the merged build. In all 4,000, their lines are those of `6a424c6`'s build, line for line, though the test store the walks keep their sessions in now runs the record store's core.
- **Doc links and graphs:** the doc links are the same broken set as before (13 in the app, 4 in cw-core). The module graphs show no cycle in the app under any of §7.1's rules, and in cw-core only the 9-module `settings` family, under the two stricter rules.
- **Windows:** the Windows lint, simulated as §8.6 describes, is clean: every Unix-only item of the test code (41 of them) compiled out, and the desktop clippy run over all targets.

Each branch was verified the same way at its head before it was merged, and its review's last check ran again on its fixes (§9.2, §9.3).

| Suite | §8.4's final (`4f6d29d`) | The store (`aa6af38`) | The lifecycle (`6a424c6`) | Merged (`984d718`) |
|---|---|---|---|---|
| `cw-core` tests | 1,404 | 1,404 | 1,412 | 1,412 |
| `cw-core` doctests | 1 | 1 | 1 | 1 |
| desktop app tests | 1,695 | 1,791 | 1,740 | 1,836 |
| silent (mobile) build tests | 1,564 | 1,660 | 1,609 | 1,705 |

All counts are tests passed, none failed. The store's branch starts from `2207d65`, and the lifecycle's from `ab0cbf7`, whose fix to the computer's sign-in (`44dbf48`) added one test to each app suite.

**Run only by CI:**
- the iOS type-check;
- the Android type-check's build of cpal's oboe C++, which the stand-in compiler here skips;
- desktop lint on macOS, and on Windows as it really builds;
- the web site's build, with its browser checks.

CI runs them on a pull request, not on a push to this branch.
