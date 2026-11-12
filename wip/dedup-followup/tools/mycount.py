#!/usr/bin/env python3
"""Independent production-line counter for the fact check.

Usage: mycount.py REV [--per-file] [--items] [PATH...]
Walks each crate's module tree from its root (src/main.rs, crates/cw-core/src/lib.rs),
so a file is test-only exactly when some module on its path is declared under a cfg
that implies `test` (or the file says #![cfg(test)]).  Inside other files, every item
whose cfg implies `test` is left out with the doc comments and attributes just above
it; an item's end is found by its own grammar (a field or variant ends at its comma).
Prints per crate: all lines of its .rs files, production lines, production code lines
(non-blank once comments are out, strings kept), production doc lines (/// or //!)."""
import re, sys, subprocess, bisect

REPO = '/home/user/dust'
ROOTS = {'src': 'src/main.rs', 'crates/cw-core/src': 'crates/cw-core/src/lib.rs'}


def git(*a):
    return subprocess.run(['git', *a], capture_output=True, text=True, cwd=REPO).stdout


def lex(src):
    """Return (bl, co): bl has comments, strings and chars blanked; co has comments
    blanked and strings kept.  Newlines are kept in both."""
    bl = []; co = []; i = 0; n = len(src)
    def sp(s): return ''.join('\n' if c == '\n' else ' ' for c in s)
    raw = re.compile(r'(?:b|c)?r(#*)"')
    chr_ = re.compile(r"b?'(\\(x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]+\}|.)|[^\\'\n])'")
    while i < n:
        c = src[i]
        prev_ident = i > 0 and (src[i-1].isalnum() or src[i-1] == '_')
        if src.startswith('//', i):
            j = src.find('\n', i); j = n if j == -1 else j
            s = sp(src[i:j]); bl.append(s); co.append(s); i = j; continue
        if src.startswith('/*', i):
            d = 1; j = i + 2
            while j < n and d > 0:
                if src.startswith('/*', j): d += 1; j += 2
                elif src.startswith('*/', j): d -= 1; j += 2
                else: j += 1
            s = sp(src[i:j]); bl.append(s); co.append(s); i = j; continue
        m = raw.match(src, i)
        if m and not prev_ident:
            end = '"' + m.group(1); j = src.find(end, m.end())
            j = n if j == -1 else j + len(end)
            bl.append(sp(src[i:j])); co.append(src[i:j]); i = j; continue
        if c == '"' or (c in 'bc' and i + 1 < n and src[i+1] == '"' and not prev_ident):
            j = i + (1 if c == '"' else 2)
            while j < n:
                if src[j] == '\\': j += 2; continue
                if src[j] == '"': j += 1; break
                j += 1
            bl.append(sp(src[i:j])); co.append(src[i:j]); i = j; continue
        if c == "'" or (c == 'b' and i + 1 < n and src[i+1] == "'" and not prev_ident):
            m = chr_.match(src, i)
            if m:
                bl.append(' ' * (m.end() - i)); co.append(src[i:m.end()]); i = m.end(); continue
        bl.append(c); co.append(c); i += 1
    return ''.join(bl), ''.join(co)


def match_close(bl, k):
    """k at an opener; index of its matching closer (all three kinds tracked)."""
    d = 0
    for x in range(k, len(bl)):
        ch = bl[x]
        if ch in '([{': d += 1
        elif ch in ')]}':
            d -= 1
            if d == 0: return x
    return len(bl) - 1


def split_args(s):
    out = []; d = 0; cur = ''
    for ch in s:
        if ch == '(': d += 1
        if ch == ')': d -= 1
        if ch == ',' and d == 0: out.append(cur); cur = ''; continue
        cur += ch
    if cur.strip(): out.append(cur)
    return [x.strip() for x in out]


def implies_test(pred):
    pred = pred.strip()
    if pred == 'test': return True
    m = re.match(r'^(all|any|not)\s*\((.*)\)$', pred, re.S)
    if not m: return False
    args = split_args(m.group(2))
    if m.group(1) == 'all': return any(implies_test(a) for a in args)
    if m.group(1) == 'any': return bool(args) and all(implies_test(a) for a in args)
    return False


ATTR = re.compile(r'#!?\[')


def skip_ws_attrs(bl, j):
    """Skip whitespace and outer attributes from j; return index of the item's first token."""
    n = len(bl)
    while True:
        while j < n and bl[j].isspace(): j += 1
        if bl.startswith('#[', j):
            j = match_close(bl, j + 1) + 1; continue
        return j


ITEM_KW = re.compile(r'(pub\s*(\([^)]*\))?\s*)?((const|async|unsafe|default|extern)\s+)*(fn|impl|struct|enum|trait|mod|union|macro_rules\s*!)\b')
STMT_KW = re.compile(r'(pub\s*(\([^)]*\))?\s*)?(use|type|const|static|let|extern\s+crate)\b')


def item_end(bl, k):
    """k: first token of an item (or field, variant, arm, statement); its last index."""
    n = len(bl)
    head = bl[k:k + 300]
    if bl[k] == '{':
        return match_close(bl, k)
    if ITEM_KW.match(head):
        d = 0; x = k
        while x < n:
            ch = bl[x]
            if ch in '([': d += 1
            elif ch in ')]': d -= 1
            elif ch == ';' and d == 0: return x
            elif ch == '{' and d == 0: return match_close(bl, x)
            x += 1
        return n - 1
    if STMT_KW.match(head):
        d = 0; x = k
        while x < n:
            ch = bl[x]
            if ch in '([{': d += 1
            elif ch in ')]}': d -= 1
            elif ch == ';' and d == 0: return x
            x += 1
        return n - 1
    if re.match(r'(if|while|for|loop|match|unsafe)\b', head):
        # control-flow expression statement: ends with its block, or its last
        # `else` block for an `if`
        x = k; d = 0
        while x < n:
            ch = bl[x]
            if ch in '([': d += 1
            elif ch in ')]': d -= 1
            elif ch == '{' and d == 0:
                e = match_close(bl, x)
                y = e + 1
                while y < n and bl[y].isspace(): y += 1
                if head.startswith('if') and re.match(r'else\b', bl[y:y+5]):
                    x = y + 4; continue
                return e
            x += 1
        return n - 1
    m = re.match(r'[\w:]+\s*!\s*', head)
    if m:  # macro invocation: name! { ... }  or name!( ... );
        x = k + m.end()
        if x < n and bl[x] == '{':
            return match_close(bl, x)
        e = match_close(bl, x)
        y = e + 1
        while y < n and bl[y] in ' \t': y += 1
        return y if y < n and bl[y] == ';' else e
    # field, variant, arm, expression statement
    d = 0; ang = 0; x = k; arrow = False
    while x < n:
        ch = bl[x]
        if ch in '([{':
            if ch == '{' and d == 0 and arrow:
                return match_close(bl, x)
            d += 1
        elif ch in ')]}':
            if d == 0: return x - 1
            d -= 1
        elif d == 0 and bl.startswith('=>', x):
            arrow = True; x += 2; continue
        elif d == 0 and ch == '<' and not arrow: ang += 1
        elif d == 0 and ch == '>' and ang > 0 and bl[x-1] != '-': ang -= 1
        elif d == 0 and ch == ';': return x
        elif d == 0 and ang == 0 and ch == ',': return x
        x += 1
    return n - 1


def test_items(src, bl):
    """Line indexes (0-based) inside items whose cfg implies test, and the list of
    (first line, last line, kind) for each."""
    lines = src.split('\n')
    starts = [0]
    for l in lines: starts.append(starts[-1] + len(l) + 1)
    lineof = lambda idx: bisect.bisect_right(starts, idx) - 1
    mask = set(); spans = []
    for m in re.finditer(r'#\[\s*cfg\s*\(', bl):
        close = match_close(bl, m.end() - 1)
        pred = bl[m.end():close]
        if not implies_test(pred): continue
        attr_end = close + 1
        while attr_end < len(bl) and bl[attr_end] != ']': attr_end += 1
        k = skip_ws_attrs(bl, attr_end + 1)
        e = item_end(bl, k)
        a, b = lineof(m.start()), lineof(e)
        # doc comments and attributes just above go with the item
        while a > 0:
            t = lines[a-1].strip()
            if t.startswith('///') or (t.startswith('#[') and not t.startswith('#![')):
                a -= 1
            else:
                break
        kind = re.match(r'[^\n]{0,60}', bl[k:]).group(0).strip()
        spans.append((a, b, kind))
        mask.update(range(a, b + 1))
    return mask, spans


def file_is_inner_test(bl):
    for m in re.finditer(r'#!\[\s*cfg\s*\(', bl):
        close = match_close(bl, m.end() - 1)
        if implies_test(bl[m.end():close]): return True
    return False


def module_tree(rev, root_file, allfiles):
    """{file: is_test_only} for every file reached from root_file."""
    seen = {}
    todo = [(root_file, False)]
    while todo:
        f, t = todo.pop()
        if f in seen:
            seen[f] = seen[f] and t; continue
        src = git('show', f'{rev}:{f}')
        bl, _ = lex(src)
        t = t or file_is_inner_test(bl)
        seen[f] = t
        d, base = f.rsplit('/', 1)
        if base in ('mod.rs', 'lib.rs', 'main.rs'): here = d
        else: here = d + '/' + base[:-3]
        # walk mod declarations, tracking inline-module nesting and cfg(test) attributes
        decl = re.compile(r'((?:#\[[^\]]*\]\s*|#\[[^\n]*\n(?:[^\n]*\n)*?[^\n]*\]\s*)*)(?:pub\s*(?:\([^)]*\))?\s*)?mod\s+(\w+)\s*([;{])')
        def walk(lo, hi, path, intest):
            x = lo
            while x < hi:
                m = re.compile(r'\bmod\s+(\w+)\s*([;{])').search(bl, x, hi)
                if not m: return
                # attributes before this mod: scan back over whitespace / attributes
                s = m.start()
                y = s
                # also allow `pub(...)` before mod
                pm = re.search(r'(pub\s*(\([^)]*\))?\s*)$', bl[max(0, s-40):s])
                if pm: y = s - len(pm.group(1))
                attrs = []
                while True:
                    z = y
                    while z > 0 and bl[z-1].isspace(): z -= 1
                    if z > 0 and bl[z-1] == ']':
                        # find matching '#['
                        dd = 0; w = z - 1
                        while w >= 0:
                            if bl[w] == ']': dd += 1
                            elif bl[w] == '[':
                                dd -= 1
                                if dd == 0: break
                            w -= 1
                        if w > 0 and bl[w-1] == '#':
                            attrs.append(bl[w-1:z]); y = w - 1; continue
                    break
                gated = any(implies_test(a2[a2.index('(')+1:a2.rindex(')')]) for a2 in attrs if re.match(r'#\[\s*cfg\s*\(', a2))
                name = m.group(1)
                if m.group(2) == ';':
                    cand = [f'{path}/{name}.rs', f'{path}/{name}/mod.rs']
                    hit = [c for c in cand if c in allfiles]
                    if hit: todo.append((hit[0], intest or gated))
                    else: print(f'warning: {f}: mod {name}; not found ({cand})', file=sys.stderr)
                    x = m.end()
                else:
                    ob = m.end() - 1; cb = match_close(bl, ob)
                    walk(ob + 1, cb, path + '/' + name, intest or gated)
                    x = cb + 1
        walk(0, len(bl), here, t)
    return seen


def count_file(src, test_only):
    lines = src.split('\n')
    if src.endswith('\n'): lines = lines[:-1]
    n = len(lines)
    if test_only: return n, 0, 0, 0, []
    bl, co = lex(src)
    mask, spans = test_items(src, bl)
    col = co.split('\n')
    prod = [x for x in range(n) if x not in mask]
    code = sum(1 for x in prod if col[x].strip())
    doc = sum(1 for x in prod if lines[x].strip().startswith(('///', '//!')))
    return n, len(prod), code, doc, spans


def main():
    args = sys.argv[1:]
    rev = args.pop(0)
    per_file = '--per-file' in args; items = '--items' in args
    args = [a for a in args if not a.startswith('--')]
    allfiles = set(f for f in git('ls-tree', '-r', '--name-only', rev).split('\n') if f.endswith('.rs'))
    tree = {}
    for crate, root in ROOTS.items():
        tree.update(module_tree(rev, root, allfiles))
    for f in sorted(allfiles):
        if f.startswith(('src/', 'crates/cw-core/src/')) and f not in tree:
            print(f'warning: {f} is in no module tree', file=sys.stderr)
    paths = args or list(ROOTS)
    tot = {}
    for f in sorted(allfiles):
        sel = [p for p in paths if f == p or f.startswith(p.rstrip('/') + '/')]
        if not sel: continue
        crate = 'crates/cw-core/src' if f.startswith('crates/cw-core/src/') else 'src'
        src = git('show', f'{rev}:{f}')
        n, p, c, d, spans = count_file(src, tree.get(f, True))
        if per_file:
            print(f'{f}\t{n}\t{p}\t{c}\t{d}' + ('\t(test file)' if tree.get(f, True) else ''))
        if items:
            for a, b, kind in spans:
                print(f'  item {f}:{a+1}-{b+1}\t{b-a+1}\t{kind}')
        t = tot.setdefault(crate, [0, 0, 0, 0])
        for i, v in enumerate((n, p, c, d)): t[i] += v
    g = [0, 0, 0, 0]
    for crate, t in tot.items():
        print(f'CRATE {crate}\tall {t[0]}\tprod {t[1]}\tcode {t[2]}\tdoc {t[3]}\ttest(all-prod) {t[0]-t[1]}')
        for i in range(4): g[i] += t[i]
    print(f'TOTAL\tall {g[0]}\tprod {g[1]}\tcode {g[2]}\tdoc {g[3]}\ttest(all-prod) {g[0]-g[1]}')


main()
