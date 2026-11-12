#!/usr/bin/env python3
"""Module dependency graph of one Rust crate, from the paths its production code names.

Usage:
  modgraph2.py <src-dir> [--depth N] [--rev REV] [--evidence K] [--edges] [--from A] [--to B]
               [--testonly]

  <src-dir>   the crate's src directory (holds main.rs or lib.rs)
  --depth N   nodes are module paths cut to N segments (default 1; 99 = every module)
  --rev REV   read the files at a git revision instead of the working tree
  --evidence  how many places to print per edge inside a cycle (default 4)
  --edges     print every edge, not only those inside cycles
  --from/--to print only edges from/to nodes with this prefix (implies --edges)
  --testonly  print the modules found to be test-only, and the cut regions' count
  --strict    the strict rule: a path that reaches its item through a re-export (or glob) in a
              module other than its own and the crate root also depends on that module, and a
              component counts as "parent and child only" only when every edge in it joins a module
              and one of its own ancestors (the default rule drops child-to-ancestor edges instead)
  --lineage   only the classification half of --strict: edges go to defining modules as by default,
              but a component counts as "parent and child only" only when every edge in it joins a
              module and one of its own ancestors (so a cycle through a parent's own items counts)
  --methods   also add edges for calls of inherent methods: `Type::name(` resolved to the module
              that holds that impl block, and `.name(` when exactly one inherent method in the crate
              has that name, no trait in the crate declares it, and it is not a common std name

How it reads the code:
  * comments, strings and char literals are masked first, so braces and paths in them are not seen;
  * code gated on a cfg that can only hold under test (`test`, `all(test, ..)`) is cut: items,
    statements, fields, arms; a module declared under such a cfg is test-only, with its children;
  * every `use` tree is expanded, and every `crate::`/`super::`/`self::` path and every path that
    starts with a name in scope (a child module or an imported name) is resolved to the module that
    defines its item, following `use` aliases and glob imports (so `crate::Name` re-exported at the
    root resolves to the module that defines `Name`). Inline `mod x { .. }` blocks are modules.
  * a path whose item cannot be found resolves to the last module reached.

Cycles are the strongly connected components with more than one node. A component whose edges
all join a module and one of its own ancestors is listed apart ("parent and child only"): a
module and its children see each other's private items, so they are one unit in Rust.
"""
import os
import re
import subprocess
import sys
from collections import defaultdict

# ---------------------------------------------------------------- arguments

args = sys.argv[1:]
if not args or args[0] in ("-h", "--help"):
    print(__doc__)
    sys.exit(0)
SRC = args[0].rstrip("/")
DEPTH = 1
REV = None
EVIDENCE = 4
SHOW_EDGES = False
FROM = None
TO = None
TESTONLY = False
CUTS = None
METHODS = False
STRICT = False
LINEAGE = False
i = 1
while i < len(args):
    a = args[i]
    if a == "--depth":
        DEPTH = int(args[i + 1]); i += 2
    elif a == "--rev":
        REV = args[i + 1]; i += 2
    elif a == "--evidence":
        EVIDENCE = int(args[i + 1]); i += 2
    elif a == "--edges":
        SHOW_EDGES = True; i += 1
    elif a == "--from":
        FROM = args[i + 1]; SHOW_EDGES = True; i += 2
    elif a == "--to":
        TO = args[i + 1]; SHOW_EDGES = True; i += 2
    elif a == "--testonly":
        TESTONLY = True; i += 1
    elif a == "--cuts":
        CUTS = args[i + 1]; i += 2
    elif a == "--methods":
        METHODS = True; i += 1
    elif a == "--strict":
        STRICT = True; LINEAGE = True; i += 1
    elif a == "--lineage":
        LINEAGE = True; i += 1
    else:
        sys.exit(f"unknown argument {a}")

# ---------------------------------------------------------------- reading files


def git_root(path):
    return subprocess.check_output(["git", "-C", path, "rev-parse", "--show-toplevel"], text=True).strip()


def list_files():
    """Paths relative to SRC of every .rs file, and a reader."""
    if REV is None:
        out = []
        for dp, _, fns in os.walk(SRC):
            for f in fns:
                if f.endswith(".rs"):
                    out.append(os.path.relpath(os.path.join(dp, f), SRC))
        return sorted(out), lambda rel: open(os.path.join(SRC, rel), encoding="utf-8").read()
    root = git_root(SRC)
    prefix = os.path.relpath(os.path.abspath(SRC), root)
    names = subprocess.check_output(["git", "-C", root, "ls-tree", "-r", "--name-only", REV, "--", prefix], text=True).split("\n")
    out = [os.path.relpath(n, prefix) for n in names if n.endswith(".rs")]

    def read(rel):
        return subprocess.check_output(["git", "-C", root, "show", f"{REV}:{prefix}/{rel}"], text=True)

    return sorted(out), read


# ---------------------------------------------------------------- masking


def mask(text):
    """Blank out comments, string and char literals (newlines kept, so offsets and lines hold)."""
    out = list(text)
    n = len(text)
    i = 0

    def blank(a, b):
        for k in range(a, b):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        c = text[i]
        if c == "/" and i + 1 < n and text[i + 1] == "/":
            j = text.find("\n", i)
            j = n if j < 0 else j
            blank(i, j)
            i = j
            continue
        if c == "/" and i + 1 < n and text[i + 1] == "*":
            depth = 1
            j = i + 2
            while j < n and depth:
                if text.startswith("/*", j):
                    depth += 1; j += 2
                elif text.startswith("*/", j):
                    depth -= 1; j += 2
                else:
                    j += 1
            blank(i, j)
            i = j
            continue
        # raw strings: r"..", r#".."#, br#".."#
        m = re.match(r'b?r(#*)"', text[i:i + 300]) if c in "rb" and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")) else None
        if m:
            hashes = m.group(1)
            end = text.find('"' + hashes, i + m.end())
            end = n if end < 0 else end + 1 + len(hashes)
            blank(i, end)
            i = end
            continue
        if c == '"' or (c == "b" and i + 1 < n and text[i + 1] == '"' and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_"))):
            j = i + (2 if c == "b" else 1)
            while j < n:
                if text[j] == "\\":
                    j += 2
                    continue
                if text[j] == '"':
                    j += 1
                    break
                j += 1
            blank(i, j)
            i = j
            continue
        if c == "'":
            # char literal or lifetime/label
            if i + 1 < n and text[i + 1] == "\\":
                j = i + 2
                while j < n and text[j] != "'":
                    j += 1
                blank(i, j + 1)
                i = j + 1
                continue
            if i + 2 < n and text[i + 2] == "'":
                blank(i, i + 3)
                i += 3
                continue
            i += 1
            continue
        i += 1
    return "".join(out)


# ---------------------------------------------------------------- cfg(test) cutting

CFG_ATTR = re.compile(r"#\s*\[\s*cfg\s*\(")


def matching(text, i, open_ch, close_ch):
    """Index just past the bracket that closes the one at i."""
    depth = 0
    n = len(text)
    while i < n:
        ch = text[i]
        if ch == open_ch:
            depth += 1
        elif ch == close_ch:
            depth -= 1
            if depth == 0:
                return i + 1
        i += 1
    return n


def split_args(s):
    parts, depth, cur = [], 0, ""
    for ch in s:
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
        if ch == "," and depth == 0:
            parts.append(cur)
            cur = ""
        else:
            cur += ch
    if cur.strip():
        parts.append(cur)
    return [p.strip() for p in parts]


def implies_test(expr):
    """True when the cfg predicate can only hold under `test`."""
    expr = expr.strip()
    if expr == "test":
        return True
    m = re.match(r"(all|any|not)\s*\((.*)\)\s*$", expr, re.S)
    if not m:
        return False
    kind, inner = m.group(1), m.group(2)
    parts = split_args(inner)
    if kind == "all":
        return any(implies_test(p) for p in parts)
    if kind == "any":
        return bool(parts) and all(implies_test(p) for p in parts)
    return False


ITEM_KW = {"fn", "impl", "mod", "struct", "enum", "trait", "union", "macro_rules"}
DECL_KW = {"const", "static", "type", "use", "extern", "let"}
BLOCK_KW = {"if", "match", "for", "while", "loop", "unsafe"}
CLOSE = {"(": ")", "[": "]", "{": "}"}


def skip_ws(text, i):
    n = len(text)
    while i < n and text[i].isspace():
        i += 1
    return i


def block_expr_end(text, j):
    """End of a block-like expression (if/match/loop/...) starting at j, with else chains."""
    n = len(text)
    depth = 0
    k = j
    while k < n:
        ch = text[k]
        if ch in "([":
            depth += 1
        elif ch in ")]":
            depth -= 1
        elif ch == "{" and depth == 0:
            e = matching(text, k, "{", "}")
            nxt = skip_ws(text, e)
            if text.startswith("else", nxt) and not (text[nxt + 4:nxt + 5].isalnum() or text[nxt + 4:nxt + 5] == "_"):
                k = nxt + 4
                continue
            if nxt < n and text[nxt] == ";":
                return nxt + 1
            return e
        elif ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth < 0:
                return k
        k += 1
    return n


def item_end(text, i):
    """End of the item, statement, field, variant or arm that starts at i (attributes skipped)."""
    n = len(text)
    if text.startswith("{", i):
        e = matching(text, i, "{", "}")
        nxt = skip_ws(text, e)
        return nxt + 1 if nxt < n and text[nxt] == ";" else e
    m = re.match(r"(pub\s*(\([^)]*\))?\s*)?((unsafe|async|const|default|extern\s*(\"[^\"]*\")?)\s+(?=[a-z]))*", text[i:i + 200])
    j = i + (m.end() if m else 0)
    # a macro invocation: path!(..) / path![..] / path!{..}
    mm = re.match(r"(?:[A-Za-z_][A-Za-z0-9_]*\s*::\s*)*([A-Za-z_][A-Za-z0-9_]*)\s*!\s*", text[j:j + 200])
    if mm and mm.group(1) != "macro_rules":
        k = j + mm.end()
        if k < n and text[k] in CLOSE:
            e = matching(text, k, text[k], CLOSE[text[k]])
            nxt = skip_ws(text, e)
            if nxt < n and text[nxt] == ";":
                return nxt + 1
            if text[k] == "{":
                return e
            j = e  # an expression that goes on: scan on from here
    w = re.match(r"[A-Za-z_][A-Za-z0-9_]*", text[j:j + 40])
    word = w.group(0) if w else ""
    if word in BLOCK_KW:
        return block_expr_end(text, j)
    item_mode = word in ITEM_KW or (word == "const" and re.match(r"const\s+fn\b", text[j:j + 20]) is not None)
    decl = word in DECL_KW and not item_mode
    field = not item_mode and not decl and re.match(r"[A-Za-z_][A-Za-z0-9_]*\s*:(?!:)", text[j:j + 80]) is not None
    depth = 0
    angle = 0
    k = j
    while k < n:
        ch = text[k]
        if ch in "([{":
            if ch == "{" and depth == 0 and item_mode:
                return matching(text, k, "{", "}")
            depth += 1
        elif ch in ")]}":
            depth -= 1
            if depth < 0:
                return k  # the enclosing block closes: stop before it
        elif ch == "<" and field and depth == 0:
            angle += 1
        elif ch == ">" and field and depth == 0 and angle > 0 and text[k - 1] not in "-=":
            angle -= 1
        elif ch == ";" and depth == 0:
            return k + 1
        elif ch == "," and depth == 0 and angle == 0 and not item_mode and not decl:
            return k + 1
        elif ch == "=" and text.startswith("=>", k) and depth == 0 and not item_mode and not decl:
            nxt = skip_ws(text, k + 2)
            if nxt < n and text[nxt] == "{":
                e = matching(text, nxt, "{", "}")
                after = skip_ws(text, e)
                return after + 1 if after < n and text[after] == "," else e
            k += 2
            continue
        k += 1
    return n


def cut_tests(masked):
    """Blank every region gated on a test-only cfg. Returns (text, regions)."""
    out = list(masked)
    regions = []
    pos = 0
    while True:
        m = CFG_ATTR.search(masked, pos)
        if not m:
            break
        start = m.start()
        paren = masked.index("(", m.start())
        close = matching(masked, paren, "(", ")")
        pred = masked[paren + 1:close - 1]
        bracket_end = masked.find("]", close - 1) + 1
        pos = bracket_end
        if not implies_test(pred):
            continue
        # skip further attributes
        j = skip_ws(masked, bracket_end)
        while masked.startswith("#", j):
            b = masked.index("[", j)
            j = skip_ws(masked, matching(masked, b, "[", "]"))
        end = item_end(masked, j)
        for k in range(start, end):
            if out[k] != "\n":
                out[k] = " "
        regions.append((start, end))
        pos = max(pos, end)
    return "".join(out), regions


# ---------------------------------------------------------------- scopes, items and uses

IDENT = r"[A-Za-z_][A-Za-z0-9_]*"
MOD_DECL = re.compile(r"(?:pub\s*(?:\([^)]*\))?\s*)?\bmod\s+(" + IDENT + r")\s*([;{])")
ITEM_DEF = re.compile(r"(?:pub\s*(?:\([^)]*\))?\s*)?(?:(?:unsafe|async|const|extern\s*(?:\"[^\"]*\")?)\s+)*\b(fn|struct|enum|trait|type|const|static|union)\s+(" + IDENT + r")")
MACRO_DEF = re.compile(r"\bmacro_rules!\s*(" + IDENT + r")")
USE_KW = re.compile(r"(?<![A-Za-z0-9_])use\s+")


def line_of(text, idx):
    return text.count("\n", 0, idx) + 1


class Module:
    def __init__(self, path, file):
        self.path = path            # tuple of segments
        self.file = file            # file relative to SRC
        self.children = {}          # name -> path (file or inline)
        self.items = set()          # names defined at top level
        self.aliases = {}           # name -> (base module path, segments)
        self.globs = []             # (base module path, segments)
        self.declared = set()       # child module names declared in production code


modules = {}       # path -> Module
file_text = {}     # rel -> (original, cut masked)


def parse_use(stmt):
    """Like parse_use_tree but keeps `as` aliases (spaces matter for them)."""
    stmt = stmt.strip()
    out = []

    def walk(prefix, s):
        s = s.strip()
        if not s:
            return
        k = s.find("{")
        if k >= 0:
            head = s[:k].strip()
            end = matching(s, k, "{", "}")
            body = s[k + 1:end - 1]
            head_segs = [p.strip() for p in head.split("::") if p.strip()]
            depth, cur = 0, ""
            for ch in body:
                if ch == "{":
                    depth += 1
                elif ch == "}":
                    depth -= 1
                if ch == "," and depth == 0:
                    walk(prefix + head_segs, cur)
                    cur = ""
                else:
                    cur += ch
            walk(prefix + head_segs, cur)
            return
        m = re.match(r"(.*?)\s+as\s+(" + IDENT + r")\s*$", s, re.S)
        alias = None
        if m:
            s, alias = m.group(1), m.group(2)
        segs = [p.strip() for p in s.split("::") if p.strip()]
        if segs and segs[0] == "":
            segs = segs[1:]
        out.append((prefix + segs, alias))

    walk([], stmt)
    return out


def module_path_of_file(rel):
    p = rel[:-3]
    if p in ("main", "lib"):
        return ()
    if p.endswith("/mod"):
        p = p[:-4]
    return tuple(p.split("/"))


def scan_file(rel, original):
    masked = mask(original)
    cut, regions = cut_tests(masked)
    file_text[rel] = (original, cut, regions)
    base = module_path_of_file(rel)
    mod = modules.setdefault(base, Module(base, rel))
    mod.file = rel
    # scopes: (start, end, path); the file scope first
    scopes = [(0, len(cut), base)]
    stack = [(len(cut), base, 0)]  # (end, path, brace depth of the scope's top level)
    depth = 0
    i = 0
    n = len(cut)
    # find inline modules and top-level items by walking braces
    tokens = re.finditer(r"[{}]|" + MOD_DECL.pattern + "|" + ITEM_DEF.pattern + "|" + MACRO_DEF.pattern, cut)
    for t in tokens:
        while stack and t.start() >= stack[-1][0] and len(stack) > 1:
            stack.pop()
        txt = t.group(0)
        if txt == "{":
            depth += 1
            continue
        if txt == "}":
            depth -= 1
            continue
        cur_end, cur_path, cur_depth = stack[-1]
        if depth != cur_depth:
            continue
        mm = MOD_DECL.match(txt)
        if mm:
            name = mm.group(1)
            m_scope = modules.setdefault(cur_path, Module(cur_path, rel))
            if mm.group(2) == ";":
                m_scope.declared.add(name)
                m_scope.children.setdefault(name, cur_path + (name,))
            else:
                # inline module: its braces start at the '{' that ends the match
                brace = t.end() - 1
                end = matching(cut, brace, "{", "}")
                child = cur_path + (name,)
                m_scope.children[name] = child
                m_scope.declared.add(name)
                modules.setdefault(child, Module(child, rel))
                scopes.append((brace, end, child))
                depth += 1  # the match took the module's opening brace
                stack.append((end, child, depth))
            continue
        mi = ITEM_DEF.match(txt)
        if mi:
            modules.setdefault(cur_path, Module(cur_path, rel)).items.add(mi.group(2))
            continue
        mc = MACRO_DEF.match(txt)
        if mc:
            modules.setdefault(cur_path, Module(cur_path, rel)).items.add(mc.group(1))
    return scopes


file_scopes = {}
use_spans = {}   # rel -> [(start, end)] of use declarations


def in_use(rel, idx):
    return any(a <= idx < b for a, b in use_spans.get(rel, ()))


def scope_at(rel, idx):
    best = None
    for (a, b, path) in file_scopes[rel]:
        if a <= idx < b and (best is None or len(path) > len(best)):
            best = path
    return best


def collect_uses(rel):
    _, cut, _ = file_text[rel]
    uses = []
    for m in USE_KW.finditer(cut):
        # skip `use` that is part of a path or identifier (e.g. `r#use`)
        start = m.start()
        before = cut[max(0, start - 2):start]
        if before.endswith("::"):
            continue
        j = m.end()
        depth = 0
        k = j
        while k < len(cut):
            ch = cut[k]
            if ch == "{":
                depth += 1
            elif ch == "}":
                depth -= 1
            elif ch == ";" and depth == 0:
                break
            k += 1
        stmt = cut[j:k]
        use_spans.setdefault(rel, []).append((start, k))
        scope = scope_at(rel, start)
        for segs, alias in parse_use(stmt):
            uses.append((scope, segs, alias, line_of(cut, start), stmt))
    return uses


# ---------------------------------------------------------------- resolution


def resolve_base(scope, segs):
    """(module path, remaining segments) for the start of a path, or None if external."""
    if not segs:
        return None
    first = segs[0]
    if first == "crate":
        return (), segs[1:]
    if first == "self":
        return scope, segs[1:]
    if first == "super":
        cur = scope
        k = 0
        while k < len(segs) and segs[k] == "super":
            cur = cur[:-1]
            k += 1
        return cur, segs[k:]
    mod = modules.get(scope)
    if mod is not None and first in mod.children:
        return mod.children[first], segs[1:]
    if mod is not None and first in mod.aliases:
        return ("alias", first)
    if mod is not None and first in mod.items:
        return scope, segs
    return None


def walk(path, segs, seen=None):
    """Follow segments from a module to the module that defines the item."""
    seen = seen or set()
    cur = path
    for idx, s in enumerate(segs):
        if s in ("*", "self"):
            return cur
        mod = modules.get(cur)
        if mod is None:
            return cur
        if s in mod.children:
            cur = mod.children[s]
            continue
        if s in mod.items:
            return cur
        if s in mod.aliases and (cur, s) not in seen:
            seen.add((cur, s))
            tgt = resolve_alias(cur, s, seen)
            if tgt is None:
                return cur
            # the alias may name a module: continue from it
            if tgt in modules and idx + 1 < len(segs):
                return walk(tgt, segs[idx + 1:], seen)
            return tgt
        for gbase in mod.globs:
            gpath = gbase
            gm = modules.get(gpath)
            if gm is not None and (s in gm.items or s in gm.children):
                return walk(gpath, segs[idx:], seen)
        return cur
    return cur


alias_target = {}


def resolve_alias(scope, name, seen=None):
    seen = seen or set()
    key = (scope, name)
    if key in alias_target:
        return alias_target[key]
    mod = modules[scope]
    base_scope, segs = mod.aliases[name]
    r = resolve_base(base_scope, segs)
    if r is None:
        alias_target[key] = None
        return None
    if r[0] == "alias":
        inner = resolve_alias(base_scope, r[1], seen | {key})
        if inner is None:
            alias_target[key] = None
            return None
        tgt = walk(inner, segs[1:], seen | {key})
    else:
        tgt = walk(r[0], r[1], seen | {key})
    alias_target[key] = tgt
    return tgt


def resolve_path(scope, segs):
    r = resolve_base(scope, segs)
    if r is None:
        return None
    if r[0] == "alias":
        tgt = resolve_alias(scope, r[1])
        if tgt is None:
            return None
        if tgt in modules and len(segs) > 1:
            return walk(tgt, segs[1:])
        return tgt
    return walk(r[0], r[1])


def walk_v(path, segs, via, seen):
    """walk(), recording each module whose imports or re-exports the path passes through."""
    cur = path
    for idx, s in enumerate(segs):
        if s in ("*", "self"):
            return cur
        mod = modules.get(cur)
        if mod is None:
            return cur
        if s in mod.children:
            cur = mod.children[s]
            continue
        if s in mod.items:
            return cur
        if s in mod.aliases and (cur, s) not in seen:
            seen.add((cur, s))
            via.append(cur)
            base_scope, asegs = mod.aliases[s]
            tgt = resolve_v(base_scope, asegs, via, seen)
            if tgt is None:
                return cur
            if tgt in modules and idx + 1 < len(segs):
                return walk_v(tgt, segs[idx + 1:], via, seen)
            return tgt
        for g in mod.globs:
            gm = modules.get(g)
            if gm is not None and (s in gm.items or s in gm.children):
                via.append(cur)
                return walk_v(g, segs[idx:], via, seen)
        return cur
    return cur


def resolve_v(scope, segs, via, seen):
    r = resolve_base(scope, segs)
    if r is None:
        return None
    if r[0] == "alias":
        name = r[1]
        if (scope, name) in seen:
            return None
        seen.add((scope, name))
        base_scope, asegs = modules[scope].aliases[name]
        tgt = resolve_v(base_scope, asegs, via, seen)
        if tgt is None:
            return None
        if tgt in modules and len(segs) > 1:
            return walk_v(tgt, segs[1:], via, seen)
        return tgt
    return walk_v(r[0], r[1], via, seen)


def hops(scope, segs):
    """Modules other than scope and the crate root whose re-exports the path goes through."""
    via = []
    resolve_v(scope, segs, via, set())
    return [v for v in dict.fromkeys(via) if v != scope and v != ()]


# ---------------------------------------------------------------- main

files, read = list_files()
for rel in files:
    file_scopes[rel] = scan_file(rel, read(rel))

# production modules: reachable from the root through declarations in uncut code
prod = set()
stack = [()]
while stack:
    p = stack.pop()
    if p in prod or p not in modules:
        continue
    prod.add(p)
    for name in modules[p].declared:
        stack.append(modules[p].children[name])
test_only = sorted(p for p in modules if p not in prod)

all_uses = {}
for rel in files:
    if module_path_of_file(rel) not in prod:
        continue
    all_uses[rel] = collect_uses(rel)
    for scope, segs, alias, line, stmt in all_uses[rel]:
        if scope not in prod or not segs or segs[-1] == "*":
            continue
        last = segs[-1]
        name = alias or (segs[-2] if last == "self" and len(segs) > 1 else last)
        target_segs = segs[:-1] if last == "self" else segs
        modules[scope].aliases.setdefault(name, (scope, target_segs))
for rel, uses in all_uses.items():
    for scope, segs, alias, line, stmt in uses:
        if scope not in prod or not segs or segs[-1] != "*":
            continue
        tgt = resolve_path(scope, segs[:-1]) if len(segs) > 1 else None
        if tgt is not None and tgt in modules:
            modules[scope].globs.append(tgt)
# resolve globs that named modules through aliases late (rare): leave as computed


def node(path):
    if not path:
        return "<root>"
    return "/".join(path[:DEPTH])


edges = defaultdict(lambda: defaultdict(set))  # a -> b -> {(text, where)}
warnings = []
PATH_RE = re.compile(r"(?<![A-Za-z0-9_:])((?:" + IDENT + r")(?:\s*::\s*(?:" + IDENT + r"|\*))+)")
KEYWORDS = {"crate", "super", "self"}

for rel, uses in all_uses.items():
    original, cut, _ = file_text[rel]
    # use declarations
    for scope, segs, alias, line, stmt in uses:
        if scope not in prod or not segs:
            continue
        s = segs[:-1] if segs[-1] in ("*", "self") else segs
        tgt = resolve_path(scope, s) if s else None
        if tgt is None:
            continue
        if tgt not in prod:
            warnings.append(f"{rel}:{line} production code names test-only {'/'.join(tgt)}: use {'::'.join(segs)}")
            continue
        a, b = node(scope), node(tgt)
        if a != b:
            edges[a][b].add(("use " + "::".join(segs), f"{rel}:{line}"))
        if STRICT:
            for v in hops(scope, s):
                if node(v) != a:
                    edges[a][node(v)].add((f"use {'::'.join(segs)} (through {'/'.join(v)})", f"{rel}:{line}"))
    # paths in code (outside use declarations, which are matched again harmlessly)
    for m in PATH_RE.finditer(cut):
        if in_use(rel, m.start()):
            continue  # use declarations were expanded above
        txt = m.group(1)
        segs = [p.strip() for p in txt.split("::")]
        first = segs[0]
        scope = scope_at(rel, m.start())
        if scope not in prod:
            continue
        mod = modules.get(scope)
        if first not in KEYWORDS and (mod is None or (first not in mod.children and first not in mod.aliases)):
            continue
        tgt = resolve_path(scope, [s for s in segs if s != "*"])
        if tgt is None:
            continue
        if tgt not in prod:
            warnings.append(f"{rel}:{line_of(cut, m.start())} production code names test-only {'/'.join(tgt)}: {txt}")
            continue
        a, b = node(scope), node(tgt)
        if a != b:
            edges[a][b].add((txt.replace(" ", ""), f"{rel}:{line_of(cut, m.start())}"))
        if STRICT:
            for v in hops(scope, [s for s in segs if s != "*"]):
                if node(v) != a:
                    edges[a][node(v)].add((f"{txt.replace(' ', '')} (through {'/'.join(v)})", f"{rel}:{line_of(cut, m.start())}"))


# ---------------------------------------------------------------- inherent method calls

STD_NAMES = set("""
new default clone len is_empty iter iter_mut into_iter get get_mut set insert remove push pop contains
map and_then unwrap unwrap_or unwrap_or_else unwrap_or_default expect ok err is_some is_none is_ok is_err
as_ref as_mut as_str as_slice to_string to_owned into from try_from try_into eq ne cmp partial_cmp
fmt hash next take skip filter find position any all count sum min max min_by max_by sort sort_by
sort_by_key sort_unstable dedup retain extend clear truncate split join first last starts_with ends_with
trim parse chars bytes lines collect fold rev zip enumerate chain flat_map filter_map last_mut first_mut
borrow borrow_mut replace swap with read write lock send recv spawn abs floor ceil round sqrt powi powf
exp ln log10 clamp signum is_finite is_nan max_by_key min_by_key copied cloned entry or_insert
or_insert_with or_default and or xor not keys values values_mut drain split_off append capacity reserve
with_capacity resize fill windows chunks step_by peekable cycle flatten inspect for_each partition
unzip product checked_add checked_sub checked_mul saturating_add saturating_sub saturating_mul
wrapping_add wrapping_sub to_vec into_boxed_slice as_bytes push_str format write_str display call
peek peek_mut apply update start stop reset finish build value values name kind label key id range
split_at get_or_insert_with as_deref as_deref_mut map_or map_or_else zip_with
""".split())

impl_methods = defaultdict(list)    # method name -> [(type module, type name, impl module)]
impl_spans = {}                     # rel -> [(start, end, type module, type name)] of inherent impls
trait_names = set()                 # method names declared or implemented for traits in the crate
IMPL_RE = re.compile(r"(?<![A-Za-z0-9_])impl\b")
TRAIT_RE = re.compile(r"(?<![A-Za-z0-9_])trait\s+" + IDENT + r"[^{;]*\{")
FN_RE = re.compile(r"(?<![A-Za-z0-9_])fn\s+(" + IDENT + r")")


def strip_generics(s):
    out, depth = [], 0
    for ch in s:
        if ch == "<":
            depth += 1
        elif ch == ">" and depth:
            depth -= 1
        elif depth == 0:
            out.append(ch)
    return "".join(out)


def block_fns(cut, open_idx):
    """Names of fns directly inside the block whose '{' is at open_idx."""
    end = matching(cut, open_idx, "{", "}")
    names = []
    depth = 0
    k = open_idx + 1
    last = k
    for m in FN_RE.finditer(cut, open_idx + 1, end):
        seg = cut[last:m.start()]
        depth += seg.count("{") - seg.count("}")
        last = m.start()
        if depth == 0:
            names.append(m.group(1))
    return names, end


for rel in all_uses:
    original, cut, _ = file_text[rel]
    for m in TRAIT_RE.finditer(cut):
        names, _ = block_fns(cut, m.end() - 1)
        trait_names.update(names)
    for m in IMPL_RE.finditer(cut):
        brace = cut.find("{", m.end())
        semi = cut.find(";", m.end())
        if brace < 0 or (0 <= semi < brace):
            continue
        header = strip_generics(cut[m.end():brace])
        header = re.sub(r"\bwhere\b.*$", "", header, flags=re.S).strip()
        names, _ = block_fns(cut, brace)
        if re.search(r"(?<![A-Za-z0-9_])for(?![A-Za-z0-9_])", header):
            trait_names.update(names)
            continue
        tm = re.match(r"((?:" + IDENT + r"\s*::\s*)*" + IDENT + r")", header)
        if not tm:
            continue
        segs = [p.strip() for p in tm.group(1).split("::")]
        scope = scope_at(rel, m.start())
        if scope not in prod:
            continue
        tmod = resolve_path(scope, segs)
        if tmod is None:
            continue
        for name in names:
            impl_methods[name].append((tmod, segs[-1], scope))
        impl_spans.setdefault(rel, []).append((brace, matching(cut, brace, "{", "}"), tmod, segs[-1]))

method_edges = 0
if METHODS:
    CALL_DOT = re.compile(r"\.\s*(" + IDENT + r")\s*(?:::\s*<[^()]*>\s*)?\(")
    CALL_PATH = re.compile(r"(?<![A-Za-z0-9_:])((?:" + IDENT + r"\s*::\s*)*" + IDENT + r")\s*::\s*(" + IDENT + r")(?![A-Za-z0-9_])(?!\s*::)")
    for rel in all_uses:
        original, cut, _ = file_text[rel]
        for m in CALL_DOT.finditer(cut):
            name = m.group(1)
            defs = impl_methods.get(name)
            if not defs or len(defs) != 1 or name in trait_names or name in STD_NAMES:
                continue
            if defs[0][0] == defs[0][2]:
                continue  # an impl beside its type: whoever holds a value of it already names that module
            scope = scope_at(rel, m.start())
            if scope not in prod:
                continue
            tmod, tname, imod = defs[0]
            a, b = node(scope), node(imod)
            if a != b:
                edges[a][b].add((f"method .{name}() of {tname}", f"{rel}:{line_of(cut, m.start())}"))
                method_edges += 1
        for m in CALL_PATH.finditer(cut):
            segs = [p.strip() for p in m.group(1).split("::")]
            name = m.group(2)
            if not impl_methods.get(name):
                continue
            scope = scope_at(rel, m.start())
            if scope not in prod:
                continue
            if segs == ["Self"]:
                inner = [s for s in impl_spans.get(rel, ()) if s[0] <= m.start() < s[1]]
                if not inner:
                    continue
                _, _, tmod, tname0 = max(inner, key=lambda s: s[0])
                segs = [tname0]
            else:
                tmod = resolve_path(scope, segs)
            if tmod is None:
                continue
            for (dmod, tname, imod) in impl_methods[name]:
                if dmod == tmod and tname == segs[-1] and imod != tmod:
                    a, b = node(scope), node(imod)
                    if a != b:
                        edges[a][b].add((f"{tname}::{name}() in a foreign impl", f"{rel}:{line_of(cut, m.start())}"))
                        method_edges += 1

if TESTONLY:
    print("test-only modules:", ", ".join("/".join(p) for p in test_only))
    print("cut regions:", sum(len(v[2]) for v in file_text.values()))
if CUTS:
    original, cut, regions = file_text[CUTS]
    for a, b in regions:
        first = original[a:b].split("\n")
        tail = original[b:b + 60].split("\n")[0]
        print(f"cut {line_of(original, a)}-{line_of(original, b)}: {first[0][:70]!r} ... {first[-1][-50:]!r} | then {tail!r}")
for w in sorted(set(warnings)):
    print("WARNING:", w)

# ---------------------------------------------------------------- strongly connected components

nodes = set(edges) | {b for a in edges for b in edges[a]}
index, low, on, stack2, sccs = {}, {}, set(), [], []
counter = [0]
sys.setrecursionlimit(100000)


def strong(v):
    index[v] = low[v] = counter[0]
    counter[0] += 1
    stack2.append(v)
    on.add(v)
    for w in edges.get(v, {}):
        if w not in index:
            strong(w)
            low[v] = min(low[v], low[w])
        elif w in on:
            low[v] = min(low[v], index[w])
    if low[v] == index[v]:
        comp = []
        while True:
            w = stack2.pop()
            on.discard(w)
            comp.append(w)
            if w == v:
                break
        sccs.append(sorted(comp))


for v in sorted(nodes):
    if v not in index:
        strong(v)


def is_ancestor(anc, desc):
    """True when node `anc` is a proper ancestor module of node `desc` (the crate root is not)."""
    if anc == "<root>" or desc == "<root>":
        return False
    pa, pd = anc.split("/"), desc.split("/")
    return len(pa) < len(pd) and pd[:len(pa)] == pa


def sccs_of(members, edge_ok):
    """Strongly connected components (size > 1) of the induced subgraph, keeping edges edge_ok(a, b)."""
    idx, lo, onst, st, out = {}, {}, set(), [], []
    cnt = [0]

    def visit(v):
        idx[v] = lo[v] = cnt[0]
        cnt[0] += 1
        st.append(v)
        onst.add(v)
        for w in edges.get(v, {}):
            if w not in members or not edge_ok(v, w):
                continue
            if w not in idx:
                visit(w)
                lo[v] = min(lo[v], lo[w])
            elif w in onst:
                lo[v] = min(lo[v], idx[w])
        if lo[v] == idx[v]:
            comp = []
            while True:
                w = st.pop()
                onst.discard(w)
                comp.append(w)
                if w == v:
                    break
            if len(comp) > 1:
                out.append(sorted(comp))

    for v in sorted(members):
        if v not in idx:
            visit(v)
    return out


def not_up(a, b):
    """An edge that is not a module naming one of its own ancestors."""
    return not is_ancestor(b, a)


cyclic = [c for c in sccs if len(c) > 1]
real, family = [], []


def lineage(a, b):
    return is_ancestor(a, b) or is_ancestor(b, a)


for c in cyclic:
    if LINEAGE:
        members = set(c)
        if all(lineage(a, b) for a in c for b in edges[a] if b in members):
            family.append(c)
        else:
            real.append(c)
        continue
    inner = sccs_of(set(c), not_up)
    if inner:
        real.extend(inner)
    else:
        family.append(c)

print(f"depth {DEPTH}: {len(nodes)} nodes, {sum(len(v) for v in edges.values())} edges, "
      f"{len(real)} cycles, {len(family)} parent-and-child-only components" + (" [strict]" if STRICT else " [lineage]" if LINEAGE else "")
      + (f" (with {method_edges} method-call references)" if METHODS else ""))
foreign = sorted({(tname, "/".join(dm) or "<root>", "/".join(im) or "<root>")
                  for defs in impl_methods.values() for (dm, tname, im) in defs if dm != im})
if METHODS:
    print("inherent impls away from their type:", "; ".join(f"{t} ({d}) in {i}" for t, d, i in foreign))


def show(c, edge_ok=lambda a, b: True):
    members = set(c)
    for a in c:
        for b in sorted(edges[a]):
            if b in members and edge_ok(a, b):
                items = sorted(edges[a][b], key=lambda x: x[1])
                names = sorted({t for t, _ in items})
                up = " (names its ancestor)" if is_ancestor(b, a) else ""
                print(f"  {a} -> {b}{up}: {len(items)} refs; {', '.join(names[:6])}{' ...' if len(names) > 6 else ''}")
                for t, w in items[:EVIDENCE]:
                    print(f"      {w}  {t}")


for c in sorted(real, key=len, reverse=True):
    print(f"\n== cycle of {len(c)}: {', '.join(c)}")
    show(c, (lambda a, b: True) if LINEAGE else not_up)
for c in sorted(family, key=len, reverse=True):
    print(f"\n== parent and child only: {', '.join(c)}")
    show(c, lambda a, b: is_ancestor(b, a))

if SHOW_EDGES:
    print("\n== edges")
    for a in sorted(edges):
        if FROM and not a.startswith(FROM):
            continue
        for b in sorted(edges[a]):
            if TO and not b.startswith(TO):
                continue
            items = sorted(edges[a][b], key=lambda x: x[1])
            print(f"  {a} -> {b}: {len(items)}; " + "; ".join(f"{w} {t}" for t, w in items[:EVIDENCE]))
