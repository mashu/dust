#!/usr/bin/env python3
"""Make the new docs/code-review.md from the repo's copy and the §9 draft.
Usage: python3 apply.py [base-doc] -> writes code-review.new.md beside this script.
The base doc defaults to the repository's docs/code-review.md; copy the result over it once no placeholder is left."""
import sys, re
import os
S = os.path.dirname(os.path.abspath(__file__)) + '/'
REPO = os.path.abspath(S + '../../..') + '/'
base = sys.argv[1] if len(sys.argv) > 1 else REPO + 'docs/code-review.md'
doc = open(base).read()
s9 = open(S + 's9-draft.md').read()
try:
    s9 = s9.replace('{{VERIFY}}', open(S + 'verify-template.md').read().strip())
except FileNotFoundError:
    pass
summary9 = open(S + 'summary9.md').read().strip()

def once(old, new):
    global doc
    n = doc.count(old)
    assert n == 1, (n, old[:90])
    doc = doc.replace(old, new)

# Summary: Redundant?
once("Structural duplication remains in a few places, the largest being the record-store algorithm written twice (desktop files and IndexedDB) and the two near-identical run runtimes (§2.3). The record store has since been made one core (§7.7).",
     "Structural duplication remained in a few places, the largest being the record-store algorithm written twice (desktop files and IndexedDB) and the two near-identical run runtimes (§2.3). Each of those two has since been made one: the record store one core under the files, a browser's database and the test store, and the run lifecycle one state machine under both kinds of run (§9).")

# Summary: Follow-up (§9), after the §8 paragraph
anchor = "The other eight defects of §5 are out of scope or left by decision (§8.3).\n"
once(anchor, anchor + "\n" + summary9 + "\n")

# §2.3: the line after the table
once("Section 7.4 records what has since been consolidated, and why the rest was left.",
     "Section 7.4 records what was consolidated next, and why the rest was left. Section 9 records how the first two rows were then made one each, but for liveness, which stays each kind's own (§9.3).")

# §6 intro
once("Items 1, 4, 13, 17 and 18 are now done, or done in part (§8).",
     "Items 1, 4, 13, 17 and 18 are now done, or done in part (§8), and items 8 and 9 are done but for one part each (§9).")

# §6 Phase 0: checks by hand for §9's work
once("- **§8's fixes:** the checks by hand that §8.4 lists.",
     "- **§8's fixes:** the checks by hand that §8.4 lists.\n- **§9's work:** a browser's history now goes through the record store's core (`4834961`), which ran here only on stand-ins for a browser's database, and the lifecycle ran only with the fake player. In a real browser with two tabs open, delete, Undo, restore and back up. On the web and on a phone, pause a session and a run by hiding the page, end one and resume the other, and look at what the history keeps.")

# §6 item 8
once("with one heartbeat owned by the runtime (runs currently depend on a mounted component's `use_ticks`).",
     "with one heartbeat owned by the runtime (runs currently depend on a mounted component's `use_ticks`). *Done but for the heartbeat (§9.3): one lifecycle for both, with an explicit stage, and one rule for a gap. A run is still seen by its screen's ticks, since one heartbeat would change when it pauses.*")

# §6 item 9
once("Split `Store` (45 methods; 36 in 0.16.9) into focused traits.",
     "Split `Store` (45 methods; 36 in 0.16.9) into focused traits. *Done but for the split of `Store` (§9.2), over a `Backend` trait of 35 items, 30 of them functions, rather than a small one.*")

# §7.4 record-store row
old_row_start = "| Record-store algorithm | `KeyOnly`/`key_of`/`keyed`/`taken_up` once, in `persist::record`, also for `state.rs`'s third copy (`94a72ea`, `87e9c93`); the move's four-rule filter, the tombstone, deletions and told-of reads shared, each store passing its own inputs (`6c2ca6a`). Since then one core,"
i = doc.index(old_row_start)
j = doc.index("\n", i)
doc = doc[:i] + "| Record-store algorithm | `KeyOnly`/`key_of`/`keyed`/`taken_up` once, in `persist::record`, also for `state.rs`'s third copy (`94a72ea`, `87e9c93`); the move's four-rule filter, the tombstone, deletions and told-of reads shared, each store passing its own inputs (`6c2ca6a`). Since then, one core, `persist::record_store`, over a `Backend` trait, for the files, a browser's database and the test store, after the differences between the copies that were defects were fixed, and a suite of 13 cases, of which the files run all 13, a browser 12 and the test store 10 (§9.2) | The protocols stay each store's own, behind the trait: durable steps and journals on the files, and one transaction with a compare-and-swap on the layout in a browser. Each store keeps its own backup, and a browser's page storage, its fallback, stays outside the core. Production code is longer, not shorter (§9.4) |" + doc[j:]

# §7.4 run-lifecycle row
old_life = "| Run lifecycle | one `AWAY_GAP_MS`, and `Lease::after` for the three timer spawns; the four claim-or-toast blocks were rewritten in one shorter let-else form, though each is still written out at its site (`cf2a5e6`) | `RunMachine` plus `LiveRun<M>`: it needs about ten hook methods, so it moves the code behind a trait rather than removing it. Unifying liveness changes behaviour: a run's time away would be judged by a root heartbeat instead of its screen's ticks, so it would pause at other moments: on a gap while its screen is unmounted, and up to a tick earlier or later |"
once(old_life, "| Run lifecycle | one `AWAY_GAP_MS`, and `Lease::after` for the three timer spawns; the four claim-or-toast blocks were rewritten in one shorter let-else form, each still written out at its site (`cf2a5e6`). Since then, at the owner's decision, one lifecycle, after the differences between the runtimes that were defects were fixed (two stay, §9.3): a `RunMachine` trait and a generic `LiveRun<M, O>` with an explicit `Stage`, the claim among what is written once (§9.3) | Liveness: a session is still seen by its heartbeat and a run by its screen's ticks. One heartbeat for both would change when a run pauses: a run would be watched for a gap from its start rather than from its screen's first tick, what keeps it seen would change for one kind or the other, and its 250 ms beat would run from its start rather than from when its screen mounts. The trait moves each kind's half behind hooks rather than removing it, so the code is longer (§9.4) |")

# §7.6 items 8 and 9
once("- **Item 8:** only the small consolidations above. A `RunMachine` trait would not make the code shorter, and one heartbeat for both runtimes would change when a run pauses.",
     "- **Item 8:** at first, only the small consolidations above. A `RunMachine` trait would not make the code shorter, and one heartbeat for both runtimes would change when a run pauses. The owner then chose the trait for an explicit lifecycle, at the cost of longer code. The heartbeat is still not shared (§9.3).")
once("rather than redesigned (§7.4, §7.7). `Store` is not split.",
     "rather than redesigned (§7.4, §9.2). `Store` is not split.")

# remove §7.7
a = doc.index("### 7.7 The record store as one core\n")
b = doc.index("## 8. Follow-up: the deferred defects fixed\n")
doc = doc[:a] + doc[b:]

# add §9 before Appendix A
c = doc.index("## Appendix A: examined and found sound\n")
doc = doc[:c] + s9.rstrip('\n') + "\n\n" + doc[c:]

k = doc.index('## 9. Follow-up')
assert '§7.7' not in doc[:k], 'stale §7.7 before §9'
open(S + 'code-review.new.md', 'w').write(doc)
left = re.findall(r'⟦[^⟧]*⟧|\{\{[A-Z]+\}\}', doc)
print('ok', len(doc.splitlines()), 'lines; placeholders left:', left)
