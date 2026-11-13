#!/usr/bin/env python3
"""Independent verifier helpers (written by the verifier, not shared with the author's tools).

Enclosing function is found by a brace-depth scan over comment/string-stripped text
(NOT the author's brace-at-column-0 walk). Preprocessor directive lines are blanked for
depth tracking; a #define body is attributed to "<macro NAME>".
"""
import re, os, sys, subprocess, functools
LINUX = os.environ.get("LINUX", os.path.join(os.path.dirname(os.path.abspath(__file__)), *([".."] * (3 if "/verifier/" in __file__ else 2)), "repos", "linux"))
import shutil
RG = os.environ.get("RG_BIN") or shutil.which("rg") or "rg"

def rg(args, cwd=LINUX):
    # An explicit path is always passed by callers that search the whole tree ("."), and stdin
    # is closed: ripgrep with no path argument reads stdin when stdin is a pipe or a file.
    p = subprocess.run(["rg"] + args, executable=RG, cwd=cwd, capture_output=True, text=True, stdin=subprocess.DEVNULL)
    return p.stdout

def strip(src):
    """Replace comments, strings, char literals by spaces, preserving newlines/offsets."""
    out = []
    i = 0; n = len(src)
    while i < n:
        c = src[i]
        if c == '/' and i + 1 < n and src[i+1] == '*':
            j = src.find('*/', i + 2)
            j = n if j < 0 else j + 2
            out.append(re.sub(r'[^\n]', ' ', src[i:j])); i = j
        elif c == '/' and i + 1 < n and src[i+1] == '/':
            j = src.find('\n', i)
            j = n if j < 0 else j
            out.append(' ' * (j - i)); i = j
        elif c == '"' or c == "'":
            j = i + 1
            while j < n and src[j] != c and src[j] != '\n':
                if src[j] == '\\': j += 1
                j += 1
            j = min(j + 1, n)
            out.append(c + ' ' * (j - i - 2) + c if j - i >= 2 else src[i:j]); i = j
        else:
            out.append(c); i += 1
    return ''.join(out)

@functools.lru_cache(maxsize=None)
def analyze(path):
    """Return (rawlines, strippedlines, owner[]) where owner[i] = (name, header_line) or None for line i (0-based)."""
    raw = open(os.path.join(LINUX, path), errors='replace').read()
    s = strip(raw)
    rl = raw.split('\n'); sl = s.split('\n')
    owner = [None] * len(sl)
    depth = 0
    cur = None
    pending = ''      # text at depth 0 since last ; or }
    pending_start = None
    in_pp = False; pp_macro = None
    for i, line in enumerate(sl):
        if in_pp or line.lstrip().startswith('#'):
            if not in_pp:
                m = re.match(r'\s*#\s*define\s+(\w+)', line)
                pp_macro = m.group(1) if m else None
            if pp_macro:
                owner[i] = ('<macro %s>' % pp_macro, i + 1)
            elif cur:
                owner[i] = cur
            in_pp = line.rstrip().endswith('\\')
            continue
        if cur and depth > 0:
            owner[i] = cur
        for ch_i, ch in enumerate(line):
            if ch == '{':
                if depth == 0:
                    txt = pending
                    # function if pending has ')' as last non-space char (possibly followed by attributes)
                    t = txt.strip()
                    m = None
                    if re.search(r'\)\s*(__\w+(\s*\([^)]*\))?\s*)*$', t) and not re.match(r'^(struct|union|enum)\b[^()]*$', t) and '=' not in re.sub(r'\([^()]*\)', '', t):
                        # name = identifier before first top-level '('
                        mm = re.match(r'^((?:COMPAT_)?SYSCALL_DEFINE\d|DEFINE_\w+|BPF_CALL_\d|\w*INITCALL\w*|TRACE_EVENT\w*)\s*\(\s*(\w+)', t)
                        if mm:
                            name = '%s(%s)' % (mm.group(1), mm.group(2))
                        else:
                            # find the '(' matching the final ')'
                            # scan to find first '(' at paren depth 0 whose preceding token is an identifier, skipping __attribute__((..))
                            name = None
                            pd = 0
                            for k, c2 in enumerate(t):
                                if c2 == '(':
                                    if pd == 0:
                                        pre = re.search(r'(\w+)\s*$', t[:k])
                                        if pre and pre.group(1) not in ('__attribute__', '__printf', '__releases', '__acquires', '__must_hold', '__section', '__alias', '__aligned', 'noinline_for_stack'):
                                            name = pre.group(1); break
                                    pd += 1
                                elif c2 == ')':
                                    pd -= 1
                        if name:
                            cur = (name, (pending_start if pending_start is not None else i) + 1)
                            owner[i] = cur
                            # mark header lines too
                            for h in range(pending_start if pending_start is not None else i, i):
                                owner[h] = ('<header %s>' % name, cur[1])
                    if cur is None:
                        cur = ('<file-scope block: %s>' % t[:160].replace('\n', ' '), (pending_start if pending_start is not None else i) + 1)
                        owner[i] = cur
                depth += 1
            elif ch == '}':
                depth -= 1
                if depth <= 0:
                    depth = 0; cur = None; pending = ''; pending_start = None
            elif depth == 0:
                if ch == ';':
                    pending = ''; pending_start = None
                else:
                    if not ch.isspace() and pending_start is None:
                        pending_start = i
                    pending += ch
        if depth == 0:
            pending += '\n'
    return rl, sl, owner

def enclosing(path, line):
    rl, sl, owner = analyze(path)
    o = owner[line - 1] if 0 < line <= len(owner) else None
    return o

def incomment(path, line, col_text):
    rl, sl, owner = analyze(path)
    return col_text not in sl[line - 1]

def hits(pattern, extra=()):
    out = rg(['-n', '--no-heading', '-j4', pattern] + list(extra) + ([] if any(not x.startswith('-') and not x.startswith('!') and not x.startswith('*') for x in extra) else ['.']))
    rows = []
    for row in sorted(out.splitlines()):
        try:
            path, ln, text = row.split(':', 2)
        except ValueError:
            continue
        path = path[2:] if path.startswith('./') else path
        rows.append((path, int(ln), text))
    return rows

def show(name, extra=(), pat=None):
    pat = pat or r'\b%s\b' % re.escape(name)
    for path, ln, text in hits(pat, extra):
        tag = ''
        if path.endswith(('.c', '.h')):
            rl, sl, owner = analyze(path)
            st = sl[ln - 1]
            if not re.search(pat, st):
                tag = '[COMMENT/STRING]'
            else:
                o = owner[ln - 1]
                tag = '[%s @%d]' % o if o else '[file scope]'
                if not re.search(r'\b%s\s*\(' % re.escape(name), st):
                    tag += ' NOPAREN'
        else:
            tag = '[non-C]'
        print('%s:%d: %s %s' % (path, ln, tag, ' '.join(text.split())[:150]))

if __name__ == '__main__':
    c = sys.argv[1]
    if c == 'show':
        show(sys.argv[2], sys.argv[3:])
    elif c == 'at':
        print(enclosing(sys.argv[2], int(sys.argv[3])))
