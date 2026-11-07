#!/usr/bin/env python3
"""
Offline test of track title matching logic.
Reads the interchange file and known Roon titles from the server log,
then simulates the matching logic from library.rs.
"""
import json
import re
import sys
from pathlib import Path

INTERCHANGE = Path("/Users/jdr/Dropbox/Development/rust/libretto-scratch/working/librettos/georg-solti-1981.interchange.json")
LOG = Path("/tmp/roon-rd.log")

def normalize_title(title):
    stripped = strip_opera_prefix(title)
    result = ""
    for c in stripped.lower():
        if c.isalnum() or c.isspace() or c in ('"', "'", "."):
            result += c
    return " ".join(result.split())

def strip_opera_prefix(title):
    colon_pos = title.find(": ")
    if colon_pos >= 0:
        before = title[:colon_pos]
        if any(word in before for word in ("Act ", "Akt ", "Atto ")):
            return title[colon_pos + 2:]
    return title

def extract_number_prefix(title):
    for pat in ("no. ", "nr. ", "n. "):
        if title.startswith(pat):
            rest = title[len(pat):]
            num = ""
            for c in rest:
                if c.isdigit():
                    num += c
                else:
                    break
            if num:
                return pat + num
    return None

def extract_quoted(s):
    for q in ('"', "'"):
        start = s.find(q)
        if start >= 0:
            rest = s[start + 1:]
            end = rest.find(q)
            if end >= 0:
                return rest[:end]
    return None

def titles_match(a, b):
    if a == b:
        return "exact"
    if a.startswith(b) or b.startswith(a):
        return "prefix"
    a_num = extract_number_prefix(a)
    b_num = extract_number_prefix(b)
    if a_num and b_num and a_num == b_num:
        strip_q = lambda w: w.replace('"', '').replace("'", "")
        a_words = set(strip_q(w) for w in a.split() if len(strip_q(w)) >= 4)
        b_words = set(strip_q(w) for w in b.split() if len(strip_q(w)) >= 4)
        shared = a_words & b_words
        if len(shared) >= 1:
            return "number+keyword(shared={})".format(shared)
    overture_words = ["sinfonia", "overture", "ouverture", "ouvertura", "preludio"]
    a_ov = any(w in a for w in overture_words)
    b_ov = any(w in b for w in overture_words)
    if a_ov and b_ov:
        return "overture"
    aq = extract_quoted(a)
    bq = extract_quoted(b)
    if aq and bq and aq == bq and len(aq) >= 5:
        return 'quoted("{}")'.format(aq)
    # Substring fallback
    def strip_num(s):
        s = s.replace('"', '').replace("'", "")
        pfx = extract_number_prefix(s)
        if pfx:
            return s[len(pfx):].strip()
        return s.strip()
    a_core = strip_num(a)
    b_core = strip_num(b)
    shorter, longer = (a_core, b_core) if len(a_core) <= len(b_core) else (b_core, a_core)
    if len(shorter) >= 8 and shorter in longer:
        return "substring"
    return None

def main():
    # Load interchange file
    with open(INTERCHANGE) as f:
        data = json.load(f)
    ic_tracks = [(t.get("disc_number"), t.get("track_number"), t["title"]) for t in data["tracks"]]

    # Extract unique Roon titles from log
    roon_titles = set()
    if LOG.exists():
        for line in LOG.read_text().splitlines():
            m = re.search(r'match_track:.*title=Some\("([^"]+)"\)', line)
            if m:
                t = m.group(1)
                if "rolling stone" not in t.lower():  # skip non-Figaro
                    roon_titles.add(t)

    print("=== Interchange Tracks ({}) ===".format(len(ic_tracks)))
    print()
    for disc, track, title in ic_tracks:
        norm = normalize_title(title)
        print("  d{}t{:2d}  {}".format(disc, track, title[:80]))
        print("          norm: {}".format(norm[:80]))
    print()

    print("=== Roon Titles from Log ({}) ===".format(len(roon_titles)))
    for rt in sorted(roon_titles):
        norm = normalize_title(rt)
        print("  Roon: {}".format(rt[:90]))
        print("  norm: {}".format(norm[:90]))
    print()

    # Test each Roon title against all interchange tracks
    print("=== Match Results ===")
    print()
    for rt in sorted(roon_titles):
        rt_norm = normalize_title(rt)
        matched = False
        for disc, track, ic_title in ic_tracks:
            ic_norm = normalize_title(ic_title)
            method = titles_match(rt_norm, ic_norm)
            if method:
                print("  MATCH  d{}t{:2d}  {}".format(disc, track, method))
                print("    Roon: {}".format(rt[:90]))
                print("    IC:   {}".format(ic_title[:90]))
                matched = True
                break
        if not matched:
            print("  FAIL")
            print("    Roon: {}".format(rt[:90]))
            print("    norm: {}".format(rt_norm[:90]))
            # Show closest candidates
            print("    --- Closest interchange tracks ---")
            for disc, track, ic_title in ic_tracks:
                ic_norm = normalize_title(ic_title)
                # Show any that share significant words
                rt_words = set(w for w in rt_norm.split() if len(w) >= 4)
                ic_words = set(w for w in ic_norm.split() if len(w) >= 4)
                shared = rt_words & ic_words
                if len(shared) >= 2:
                    print("      d{}t{:2d} shared={} | {}".format(disc, track, shared, ic_title[:70]))
        print()

if __name__ == "__main__":
    main()
