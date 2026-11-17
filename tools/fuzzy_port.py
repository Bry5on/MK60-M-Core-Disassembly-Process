#!/usr/bin/env python3
"""Fuzzy cross-image symbol porting by masked-halfword n-gram containment.

The second layer under tools/port_symbols.py.  The exact port needs the whole masked
instruction stream of a function to hash identically in both images; a variant that added
a bounds check, dropped a call or re-ordered two statements breaks that for the entire
function although nine tenths of its code is still the same.  This scorer compares the
functions by the SET OF n-GRAMS (default 6 consecutive masked halfwords) of their body, as
written by tools/ghidra/scripts/DumpMasked.java (one line per function:
    entry<TAB>nInstr<TAB>name<TAB>hw0 hw1 hw2 …
same masking as ExportSigs.java: pool-slot and branch displacements dropped), so a local
edit costs only the n-grams that straddle it.

For every still-generic (FUN_/LAB_/…) TARGET function with at least --min-instr (20)
instructions it finds the NAMED source function whose n-gram set explains the largest
fraction of the target's n-grams,
    score = |T ∩ S| / |T|
and emits `addr<TAB>name<TAB>method` rows (for ApplyPortedNames.java; the method column
carries the score and the source so it lands in the plate comment) for matches that are
  - at least --min-score (0.60),
  - UNIQUE: the runner-up source is at least --margin (0.10) below, and the target is also the
    source function's best target (mutual best — a library leaf that two target functions
    both resemble names neither),
  - of comparable size: the source has between --size-lo (0.5) and --size-hi (2.0) times the
    target's n-gram count (a 20-instruction target whose n-grams all occur inside a
    300-instruction source is an idiom, not a twin),
  - and the source name is not already present in the target (every name lands once; a name
    the exact pass already placed is never duplicated).
Several SOURCE dumps may be given, closest relative first: all named source functions are
scored together, the best score wins, and a tie goes to the earlier source.  Each source is
`label=path`; the label is written into the method column.

Everything else — the near misses below the floor, the ambiguous pairs, the targets with no
candidate at all — goes to the --tail report for hand review.

Run isolated:
  python3 -I tools/fuzzy_port.py --src E46_nonM=src1.masked --src M3=src2.masked DST.masked \
      -o fuzzy.tsv [--n 6] [--min-instr 20] [--min-score 0.6] [--margin 0.1] [--tail fuzzy_tail.txt]
stdlib only; read-only (writes only the output files you name).
"""
import argparse, collections, sys


def is_generic(name):
    return (not name or name.startswith("FUN_") or name.startswith("LAB_")
            or name.startswith("SUB_") or name.startswith("thunk")
            or name.startswith("DAT_") or name.startswith("tbl_")
            or name.startswith("PTR_") or name.startswith("switchD_"))


def load(path):
    """-> list of (addr, nInstr, name, [halfword ints])"""
    rows = []
    with open(path) as fh:
        for ln in fh:
            p = ln.rstrip("\n").split("\t")
            if len(p) < 3:
                continue
            hws = [int(x, 16) for x in p[3].split()] if len(p) > 3 and p[3] else []
            rows.append((p[0].strip(), int(p[1]), p[2].strip(), hws))
    return rows


def grams(hws, n):
    return {tuple(hws[i:i + n]) for i in range(len(hws) - n + 1)}


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--src", action="append", required=True, metavar="LABEL=PATH",
                    help="DumpMasked output of a NAMED reference image; repeatable, closest relative first")
    ap.add_argument("dst", help="DumpMasked output of the TARGET image")
    ap.add_argument("-o", "--out", required=True, help="output addr<TAB>name<TAB>method table")
    ap.add_argument("--n", type=int, default=6, help="n-gram length in halfwords (default 6)")
    ap.add_argument("--min-instr", type=int, default=20,
                    help="only target functions with at least this many instructions (default 20)")
    ap.add_argument("--min-score", type=float, default=0.60, help="containment floor (default 0.60)")
    ap.add_argument("--margin", type=float, default=0.10,
                    help="the runner-up must score at least this much lower (default 0.10)")
    ap.add_argument("--size-lo", type=float, default=0.5, help="min source/target n-gram count ratio (default 0.5)")
    ap.add_argument("--size-hi", type=float, default=2.0, help="max source/target n-gram count ratio (default 2.0)")
    ap.add_argument("--tail", help="optional file: near misses / ambiguous / unmatched report")
    a = ap.parse_args()

    # ---- sources: one n-gram inverted index over every named function of every source ----
    sources = []                       # (label, rows)
    for spec in a.src:
        if "=" not in spec:
            sys.exit("--src wants LABEL=PATH, got %r" % spec)
        label, path = spec.split("=", 1)
        sources.append((label, load(path)))
    src_fn = []                        # idx -> (label, prio, addr, nInstr, name, gramset)
    index = collections.defaultdict(list)   # gram -> [src idx]
    src_names = set()
    for prio, (label, rows) in enumerate(sources):
        for addr, n, name, hws in rows:
            if is_generic(name) or len(hws) < a.n:
                continue
            g = grams(hws, a.n)
            if not g:
                continue
            idx = len(src_fn)
            src_fn.append((label, prio, addr, n, name, g))
            src_names.add(name)
            for gr in g:
                index[gr].append(idx)

    # ---- targets ----
    dst = load(a.dst)
    dst_named = {name for _, _, name, _ in dst if not is_generic(name)}
    targets = [(addr, n, name, grams(hws, a.n)) for addr, n, name, hws in dst
               if n >= a.min_instr and is_generic(name)]

    # best / runner-up per target (over sources of acceptable size and unused name)
    best = {}            # dst addr -> [(score, -prio, idx), ...] top two
    for addr, n, name, g in targets:
        if not g:
            continue
        hits = collections.Counter()
        for gr in g:
            for idx in index.get(gr, ()):
                hits[idx] += 1
        cands = []
        for idx, h in hits.items():
            label, prio, saddr, sn, sname, sg = src_fn[idx]
            ratio = len(sg) / float(len(g))
            if not (a.size_lo <= ratio <= a.size_hi):
                continue
            cands.append((h / float(len(g)), -prio, idx))
        cands.sort(reverse=True)
        best[addr] = cands[:2]

    # mutual best: for every source function, which target does it explain best?
    src_best = {}        # src idx -> (score, dst addr)
    for addr, cands in best.items():
        for score, _, idx in cands:
            if idx not in src_best or score > src_best[idx][0]:
                src_best[idx] = (score, addr)

    ported, tail_lines = [], []
    used_names = set(dst_named)
    for addr, n, name, g in sorted(targets, key=lambda t: -t[1]):
        cands = best.get(addr) or []
        if not cands:
            tail_lines.append(("nomatch", addr, n, "", "no source function shares a %d-gram of comparable size" % a.n))
            continue
        score, _, idx = cands[0]
        label, prio, saddr, sn, sname, sg = src_fn[idx]
        where = "%s %s @%s (%d instr)" % (label, sname, saddr, sn)
        if score < a.min_score:
            tail_lines.append(("below-floor", addr, n, "%.2f" % score, where))
            continue
        if sname in used_names:
            tail_lines.append(("name-taken", addr, n, "%.2f" % score, where))
            continue
        if len(cands) > 1 and cands[1][0] > score - a.margin and src_fn[cands[1][2]][4] != sname:
            l2, _, a2, n2, s2, _ = src_fn[cands[1][2]]
            tail_lines.append(("ambiguous", addr, n, "%.2f" % score,
                               "%s | runner-up %.2f %s %s @%s" % (where, cands[1][0], l2, s2, a2)))
            continue
        if src_best[idx][1] != addr:
            tail_lines.append(("not-mutual", addr, n, "%.2f" % score,
                               "%s prefers target %s (%.2f)" % (where, src_best[idx][1], src_best[idx][0])))
            continue
        used_names.add(sname)
        method = "fuzzy %d-gram score=%.2f src=%s:%s@%s" % (a.n, score, label, sname, saddr)
        ported.append((addr, sname, method, score, label))

    ported.sort()
    with open(a.out, "w") as fh:
        for addr, sname, method, score, label in ported:
            fh.write("%s\t%s\t%s\n" % (addr, sname, method))

    per_src = collections.Counter(p[4] for p in ported)
    kinds = collections.Counter(t[0] for t in tail_lines)
    print("fuzzy_port: targets=%d (>=%d instr, generic) ported=%d [%s] below-floor=%d ambiguous=%d "
          "not-mutual=%d name-taken=%d nomatch=%d"
          % (len(targets), a.min_instr, len(ported),
             " ".join("%s=%d" % kv for kv in sorted(per_src.items())),
             kinds["below-floor"], kinds["ambiguous"], kinds["not-mutual"], kinds["name-taken"], kinds["nomatch"]))
    print("wrote %d names -> %s" % (len(ported), a.out))
    if a.tail:
        with open(a.tail, "w") as fh:
            fh.write("# FUZZY TAIL — %d-gram containment, floor %.2f, margin %.2f; resolve by hand\n"
                     % (a.n, a.min_score, a.margin))
            fh.write("# kind\ttarget\tinstr\tscore\tbest source / note\n")
            order = {"ambiguous": 0, "not-mutual": 1, "name-taken": 2, "below-floor": 3, "nomatch": 4}
            for kind, addr, n, score, note in sorted(tail_lines, key=lambda t: (order[t[0]], -float(t[3] or 0), t[1])):
                fh.write("%s\t%s\t%d\t%s\t%s\n" % (kind, addr, n, score, note))
        print("wrote fuzzy tail -> %s" % a.tail)


if __name__ == "__main__":
    main()
