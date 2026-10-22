#!/usr/bin/env python3
"""Count what contest callers send and write the shares contest mode draws from.

Two call-history files go in, each a summary of contest logs: a call per row
and what that station sent. This counts, from them:

- the Field Day classes stations sent (`1D`, `3A`), and how many sent DX for
  their section;
- for each US state, Canadian province or territory, the share of its Field
  Day stations in each of its ARRL or RAC sections (EMA and WMA in
  Massachusetts), Texas's three sections under Texas, the District of
  Columbia under MDC with Maryland, and the Territories under each of the
  three territories;
- the power DX stations sent in the ARRL DX contest (`100`, `KW`).

The shares go to crates/cw-core/src/contest/data/counted.rs, rounded to a
tenth of a percent. Only shares survive, never a call or a name, so every
caller Dust sends is still made up. A class or a power whose share rounds to
nothing is left out; so is one Dust does not read as one (`5W`), and the
one row of the Field Day file whose section is no section (`MD`). Each
place's sections are rounded by largest remainder, so they add up to exactly
a hundred percent.

The sections and the places each covers are listed below by call area, as
the design lists them (§11.4); `contest/data/sections.rs` lists the same, and
a test there checks that the counted table and that list agree. Puerto Rico
and the Virgin Islands are sections a station's call gives (KP4, KP2), not
places it is drawn in, so no place counts them.

Inputs, used where the table is made and never committed: FDGOTA.txt, the
Field Day call history, and ARRLDXCW_USDX.txt, the ARRL DX one, as
MorseRunner Community Edition ships them.

With --check, nothing is written: the script says whether the committed
table is what these inputs give, and fails if it is not.

Usage: scripts/contest-data.py [--check] FDGOTA_TXT ARRLDX_TXT
"""

import re
import sys
from collections import Counter
from decimal import ROUND_HALF_UP, Decimal
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "crates" / "cw-core" / "src" / "contest" / "data" / "counted.rs"

# Every ARRL and RAC section, by call area, with the places it covers by the
# codes `contest/data/places.rs` uses.
SECTIONS = [
    ("1", [("CT", ["CT"]), ("EMA", ["MA"]), ("ME", ["ME"]), ("NH", ["NH"]),
           ("RI", ["RI"]), ("VT", ["VT"]), ("WMA", ["MA"])]),
    ("2", [("ENY", ["NY"]), ("NLI", ["NY"]), ("NNJ", ["NJ"]), ("NNY", ["NY"]),
           ("SNJ", ["NJ"]), ("WNY", ["NY"])]),
    ("3", [("DE", ["DE"]), ("EPA", ["PA"]), ("MDC", ["MD", "DC"]), ("WPA", ["PA"])]),
    ("4", [("AL", ["AL"]), ("GA", ["GA"]), ("KY", ["KY"]), ("NC", ["NC"]),
           ("NFL", ["FL"]), ("PR", []), ("SC", ["SC"]), ("SFL", ["FL"]),
           ("TN", ["TN"]), ("VA", ["VA"]), ("VI", []), ("WCF", ["FL"])]),
    ("5", [("AR", ["AR"]), ("LA", ["LA"]), ("MS", ["MS"]), ("NM", ["NM"]),
           ("NTX", ["TX"]), ("OK", ["OK"]), ("STX", ["TX"]), ("WTX", ["TX"])]),
    ("6", [("EB", ["CA"]), ("LAX", ["CA"]), ("ORG", ["CA"]), ("PAC", ["HI"]),
           ("SB", ["CA"]), ("SCV", ["CA"]), ("SDG", ["CA"]), ("SF", ["CA"]),
           ("SJV", ["CA"]), ("SV", ["CA"])]),
    ("7", [("AK", ["AK"]), ("AZ", ["AZ"]), ("EWA", ["WA"]), ("ID", ["ID"]),
           ("MT", ["MT"]), ("NV", ["NV"]), ("OR", ["OR"]), ("UT", ["UT"]),
           ("WWA", ["WA"]), ("WY", ["WY"])]),
    ("8", [("MI", ["MI"]), ("OH", ["OH"]), ("WV", ["WV"])]),
    ("9", [("IL", ["IL"]), ("IN", ["IN"]), ("WI", ["WI"])]),
    ("0", [("CO", ["CO"]), ("IA", ["IA"]), ("KS", ["KS"]), ("MN", ["MN"]),
           ("MO", ["MO"]), ("ND", ["ND"]), ("NE", ["NE"]), ("SD", ["SD"])]),
    ("Canada", [("AB", ["AB"]), ("BC", ["BC"]), ("GH", ["ON"]), ("MB", ["MB"]),
                ("NB", ["NB"]), ("NL", ["NL"]), ("NS", ["NS"]), ("ONE", ["ON"]),
                ("ONN", ["ON"]), ("ONS", ["ON"]), ("PE", ["PE"]), ("QC", ["QC"]),
                ("SK", ["SK"]), ("TER", ["NT", "NU", "YT"])]),
]
DX = "DX"

# A power Dust reads: watts, up to the most an amateur runs, or a kilowatt as
# written.
POWER = re.compile(r"^(?:K|KW|1K|[0-9]+)$")
POWER_MAX = 1500
# A Field Day class: 1 to 99 transmitters and a letter from A to F.
CLASS = re.compile(r"^[1-9][0-9]?[A-F]$")
# What the files write their last edit as: `# LastEdit,2026-06-26`.
LAST_EDIT = re.compile(r"^#\s*LastEdit,(\d{4})-(\d{2})-\d{2}")
MONTHS = ["January", "February", "March", "April", "May", "June", "July",
          "August", "September", "October", "November", "December"]


def read_rows(path):
    """Each row of a call-history file as a dict of its columns, by the
    names its `!!Order!!` line gives them, the call left out; and the month
    the file was last edited, as words."""
    order, rows, edited = None, [], None
    for line in Path(path).read_text(encoding="latin-1").splitlines():
        line = line.strip()
        stamp = LAST_EDIT.match(line)
        if stamp:
            edited = f"{MONTHS[int(stamp.group(2)) - 1]} {stamp.group(1)}"
        if not line or line.startswith("#"):
            continue
        fields = [field.strip() for field in line.split(",")]
        if fields[0] == "!!Order!!":
            order = fields[1:]
            continue
        if order is None:
            continue
        row = dict(zip(order, fields))
        row.pop("Call", None)
        rows.append(row)
    return rows, edited


def tenth(share):
    """A share in percent, rounded to a tenth as people round it."""
    return Decimal(share).quantize(Decimal("0.1"), rounding=ROUND_HALF_UP)


def by_count(counts, order):
    """The values of `counts`, the most counted first, ties in the order
    `order` first met them."""
    first = {value: at for at, value in reversed(list(enumerate(order)))}
    return sorted(counts, key=lambda value: (-counts[value], first[value]))


def shares(counts, order, keep):
    """Each value `keep` takes, with its share of everything counted, the
    most counted first, those that round to nothing left out."""
    total = sum(counts.values())
    kept = []
    for value in by_count(counts, order):
        share = tenth(Decimal(counts[value]) * 100 / total)
        if share > 0 and keep(value):
            kept.append((value, share))
    return kept


def largest_remainder(counts):
    """Each of `counts`, in order, as a share in tenths of a percent that
    add up to exactly a hundred: each its share rounded down, and the tenths
    left over to the largest remainders, the first of a tie first."""
    total = sum(count for _, count in counts)
    exact = [(value, Decimal(count) * 1000 / total) for value, count in counts]
    tenths = [(value, int(share)) for value, share in exact]
    left = 1000 - sum(share for _, share in tenths)
    by_remainder = sorted(
        range(len(exact)), key=lambda at: (-(exact[at][1] - tenths[at][1]), at)
    )
    for at in by_remainder[:left]:
        value, share = tenths[at]
        tenths[at] = (value, share + 1)
    return [(value, Decimal(share) / 10) for value, share in tenths]


def field_day(rows):
    """The class shares, the share of DX, and each place's section shares,
    from Field Day rows; and how many rows were counted."""
    listed = {code for _, sections in SECTIONS for code, _ in sections}
    rows = [row for row in rows if row["Sect"] in listed or row["Sect"] == DX]
    classes = Counter(row["Exch1"] for row in rows)
    order = [row["Exch1"] for row in rows]
    class_shares = shares(classes, order, CLASS.match)
    dx = tenth(Decimal(sum(row["Sect"] == DX for row in rows)) * 100 / len(rows))
    sections = Counter(row["Sect"] for row in rows)
    places = {}
    for _, listed_here in SECTIONS:
        for code, covered in listed_here:
            for place in covered:
                places.setdefault(place, []).append((code, sections[code]))
    place_shares = [(place, largest_remainder(counts)) for place, counts in places.items()]
    return class_shares, dx, place_shares, len(rows)


def arrl_dx(rows):
    """The power shares of the DX stations among ARRL DX rows, and how many
    there were."""
    powers = [row["Power"] for row in rows if row.get("Power")]
    counts = Counter(powers)
    reads = lambda power: (
        POWER.match(power) and (not power.isdigit() or 1 <= int(power) <= POWER_MAX)
    )
    return shares(counts, powers, reads), len(powers)


def wrapped(entries, indent="    ", width=100):
    """`entries`, each already Rust, packed onto lines no wider than
    `width`."""
    lines, line = [], indent
    for entry in entries:
        if line != indent and len(line) + len(entry) + 1 > width:
            lines.append(line.rstrip())
            line = indent
        line += entry + " "
    if line.strip():
        lines.append(line.rstrip())
    return lines


def rust(fd, fd_rows, fd_edited, powers, dx_rows, dx_edited):
    class_shares, dx, place_shares = fd
    pair = lambda value, share: f'("{value}", {share:.1f}),'
    lines = [
        "//! Shares counted from contest-log summaries: counts only, no calls and",
        "//! no names, each rounded to a tenth of a percent.",
        "//!",
        f"//! - The Field Day classes stations send, and the share that send DX, from",
        f"//!   {fd_rows:,} Field Day entries as last edited in {fd_edited}; and from the",
        "//!   same, each place's share of its stations in each of its sections.",
        f"//! - The power DX stations send in the ARRL DX contest, from {dx_rows:,} DX",
        f"//!   stations as last edited in {dx_edited}.",
        "//!",
        "//! Generated by `scripts/contest-data.py FDGOTA.txt ARRLDXCW_USDX.txt`.",
        "//! Run it again rather than editing this by hand.",
        "",
        "/// Each Field Day class as stations send it, with its share of them, in",
        "/// percent: those that round to nothing are left out.",
        "#[rustfmt::skip]",
        "pub(super) const CLASSES: &[(&str, f64)] = &[",
    ]
    lines += wrapped(pair(value, share) for value, share in class_shares)
    lines += [
        "];",
        "",
        "/// The share of Field Day stations that send DX for their section, in",
        "/// percent.",
        f"pub(crate) const FIELD_DAY_DX: f64 = {dx:.1f};",
        "",
        "/// Each US state, DC, and Canadian province or territory, with its",
        "/// stations' share in each of its sections, in percent, adding up to a",
        "/// hundred.",
        "#[rustfmt::skip]",
        "pub(super) const PLACE_SECTIONS: &[(&str, &[(&str, f64)])] = &[",
    ]
    for place, sections in place_shares:
        entries = [pair(code, share) for code, share in sections]
        head = f'    ("{place}", &['
        body = " ".join(entries)[:-1]
        if len(head) + len(body) + 3 <= 100:
            lines.append(f"{head}{body}]),")
        else:
            lines.append(head)
            lines += wrapped(entries, indent="        ")
            lines[-1] = lines[-1][:-1]
            lines.append("    ]),")
    lines += [
        "];",
        "",
        "/// Each power as DX stations send it in the ARRL DX contest, with its",
        "/// share of them, in percent: those that round to nothing, and those",
        "/// that are no power Dust reads, are left out.",
        "#[rustfmt::skip]",
        "pub(super) const POWERS: &[(&str, f64)] = &[",
    ]
    lines += wrapped(pair(value, share) for value, share in powers)
    lines.append("];")
    return "\n".join(lines) + "\n"


def main():
    args = sys.argv[1:]
    check = "--check" in args
    args = [arg for arg in args if arg != "--check"]
    if len(args) != 2:
        sys.exit(__doc__.strip().splitlines()[-1])
    fd_rows, fd_edited = read_rows(args[0])
    dx_rows, dx_edited = read_rows(args[1])
    class_shares, dx, place_shares, fd_counted = field_day(fd_rows)
    powers, dx_counted = arrl_dx(dx_rows)
    text = rust(
        (class_shares, dx, place_shares), fd_counted, fd_edited,
        powers, dx_counted, dx_edited,
    )
    where = OUTPUT.relative_to(ROOT)
    if check:
        if not OUTPUT.exists() or OUTPUT.read_text() != text:
            sys.exit(f"{where} is not what these files give: run the script again")
        print(f"{where} is what these files give")
        return
    OUTPUT.write_text(text)
    print(
        f"{where}: {len(class_shares)} classes and {len(place_shares)} places "
        f"from {fd_counted} Field Day rows; {len(powers)} powers from {dx_counted} DX rows"
    )


if __name__ == "__main__":
    main()
