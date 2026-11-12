"""Compile out, in a scratch copy, what a Windows build leaves out of the
tests: each unix-only test and its worker twin (mode "tests"), and with
mode "all-test-code" every unix-only item of the test modules too.
The attribute is turned into #[cfg(windows)], false here, so the compiler
drops the item as a Windows build does. Usage: probe.py <tree> <mode> <files...>"""
import re, sys
tree, mode, files = sys.argv[1], sys.argv[2], sys.argv[3:]
UNIX = re.compile(r'^(\s*)#\[cfg\((unix|all\(unix, feature = "native-runtime"\))\)\]\s*$')
TWIN = re.compile(r'^\s*[a-z_0-9]+,\s*$')
for rel in files:
    path = f"{tree}/{rel}"
    lines = open(path).read().split("\n")
    # The test module starts at the first #[cfg(test)] followed by a mod line.
    start = next(i for i in range(len(lines) - 1)
                 if lines[i].strip() == "#[cfg(test)]" and re.match(r'^\s*(pub(\([a-z]+\))? )?mod tests', lines[i + 1]))
    flipped = []
    for i in range(start, len(lines)):
        m = UNIX.match(lines[i])
        if not m:
            continue
        nxt = lines[i + 1] if i + 1 < len(lines) else ""
        is_test = nxt.strip() == "#[test]"
        is_twin = bool(TWIN.match(nxt))
        if mode == "tests" and not (is_test or is_twin):
            continue
        lines[i] = f"{m.group(1)}#[cfg(windows)]"
        what = "test" if is_test else "twin" if is_twin else "other"
        shown = lines[i + 2] if is_test else nxt
        flipped.append((i + 1, what, shown.strip()[:100]))
    open(path, "w").write("\n".join(lines))
    for line, what, nxt in flipped:
        print(f"{rel}:{line} {what}: {nxt}")
