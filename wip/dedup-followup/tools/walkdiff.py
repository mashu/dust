#!/usr/bin/env python3
"""Compare two folders of lifecycle walk traces (DUST_LIFECYCLE=<dir>), line by line.

Usage: walkdiff.py <before-dir> <after-dir> <rule>

Rules, for what a step may change:
  equal      nothing
  marks:F>T  only group marks, from one of F to one of T (letters t p h s -), in any
             section of the line (e.g. marks:->hs, marks:t>p, marks:t->p)
  mark       only the line's `mark ...` section (what the store's mark holds)
Prints every walk that changed, how many lines, and the first few changes; exit 1 when a
change breaks the rule.
"""
import os
import re
import sys

before, after, rule = sys.argv[1], sys.argv[2], sys.argv[3]
GROUP = re.compile(r"^(\S*>\S*):([tphs-]+)([\];]*)$")


def sections(line):
    """The line's sections, split at ' | '."""
    return line.split(" | ")


def marks_only(old, new, frm, to):
    """Whether `new` is `old` with only group marks changed from one of `frm` to one of `to`."""
    a, b = old.split(" "), new.split(" ")
    if len(a) != len(b):
        return False
    for x, y in zip(a, b):
        if x == y:
            continue
        mx, my = GROUP.match(x), GROUP.match(y)
        if not (mx and my) or mx.group(1) != my.group(1) or mx.group(3) != my.group(3):
            return False
        if mx.group(2) not in frm or my.group(2) not in to:
            return False
    return True


def mark_section_only(old, new):
    a, b = sections(old), sections(new)
    if len(a) != len(b):
        return False
    return all(x == y or x.startswith("mark ") and y.startswith("mark ") for x, y in zip(a, b))


bad = 0
changed = 0
files = sorted(set(os.listdir(before)) | set(os.listdir(after)))
for name in files:
    pa, pb = os.path.join(before, name), os.path.join(after, name)
    if not (os.path.exists(pa) and os.path.exists(pb)):
        print(f"{name}: only in one folder")
        bad += 1
        continue
    la, lb = open(pa).read().split("\n"), open(pb).read().split("\n")
    if la == lb:
        continue
    changed += 1
    diffs = []
    ok = len(la) == len(lb)
    for i, (x, y) in enumerate(zip(la, lb)):
        if x == y:
            continue
        diffs.append((i, x, y))
        if rule == "equal":
            ok = False
        elif rule == "mark":
            ok &= mark_section_only(x, y)
        elif rule.startswith("marks:"):
            frm, to = rule[len("marks:"):].split(">")
            ok &= marks_only(x, y, frm, to)
        else:
            sys.exit(f"no rule {rule}")
    if not ok:
        bad += 1
    print(f"{name}: {len(diffs)} lines changed{'' if ok else '  BREAKS ' + rule}")
    for i, x, y in diffs[:2]:
        print(f"  - {x[:400]}")
        print(f"  + {y[:400]}")
print(f"{changed} walks changed of {len(files)}; {bad} break the rule {rule}")
sys.exit(1 if bad else 0)
