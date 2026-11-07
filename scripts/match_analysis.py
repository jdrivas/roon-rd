#!/usr/bin/env python3
"""
Full matching analysis: 80 Roon titles vs 32 interchange tracks.
Produces a diagnostic table showing match/fail and reasons.
"""
import json
import re
import sys

INTERCHANGE = "/Users/jdr/Dropbox/Development/rust/libretto-scratch/working/librettos/georg-solti-1981.interchange.json"
ROON_TITLES = "/tmp/roon_figaro_titles.json"

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
            return "no+kw"
    overture_words = ["sinfonia", "overture", "ouverture", "ouvertura", "preludio"]
    a_ov = any(w in a for w in overture_words)
    b_ov = any(w in b for w in overture_words)
    if a_ov and b_ov:
        return "overture"
    aq = extract_quoted(a)
    bq = extract_quoted(b)
    if aq and bq and aq == bq and len(aq) >= 5:
        return "quoted"
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
        return "substr"
    return None

def find_best_match(roon_norm, ic_tracks):
    """Try matching against all interchange tracks."""
    for idx, (disc, trackn, ic_title) in enumerate(ic_tracks):
        ic_norm = normalize_title(ic_title)
        method = titles_match(roon_norm, ic_norm)
        if method:
            return idx, method
    return None, None

def diagnose_failure(roon_raw, roon_norm, ic_tracks):
    """Diagnose why a Roon title failed to match any interchange track."""
    # Check what the Roon title looks like after prefix strip
    stripped = strip_opera_prefix(roon_raw)

    # Check for recitativo-only tracks
    if "recitativo" in roon_norm or stripped.lower().startswith("recitativo"):
        # Find IC tracks containing this recit text
        roon_words = set(w for w in roon_norm.split() if len(w) >= 5)
        candidates = []
        for idx, (d, t, ic_title) in enumerate(ic_tracks):
            ic_norm = normalize_title(ic_title)
            ic_words = set(w for w in ic_norm.split() if len(w) >= 5)
            shared = roon_words & ic_words
            if len(shared) >= 2:
                candidates.append((idx, d, t, shared))
        if candidates:
            best = max(candidates, key=lambda x: len(x[3]))
            return f"recit embedded in d{best[1]}t{best[2]} (shared: {best[3]})"
        return "standalone recit, no keyword match"

    # Check for finale sections without number
    if any(w in roon_norm for w in ["finale", "scene"]):
        roon_words = set(w for w in roon_norm.split() if len(w) >= 5)
        for idx, (d, t, ic_title) in enumerate(ic_tracks):
            ic_norm = normalize_title(ic_title)
            ic_words = set(w for w in ic_norm.split() if len(w) >= 5)
            shared = roon_words & ic_words
            if len(shared) >= 2:
                return f"finale section -> d{d}t{t} (shared: {shared})"
        return "finale/scene section, no keyword match"

    # Generic: find best keyword overlap
    roon_words = set(w for w in roon_norm.split() if len(w) >= 5)
    best_overlap = 0
    best_ic = None
    for idx, (d, t, ic_title) in enumerate(ic_tracks):
        ic_norm = normalize_title(ic_title)
        ic_words = set(w for w in ic_norm.split() if len(w) >= 5)
        shared = roon_words & ic_words
        if len(shared) > best_overlap:
            best_overlap = len(shared)
            best_ic = (d, t, shared)
    if best_ic and best_overlap >= 1:
        return f"weak match d{best_ic[0]}t{best_ic[1]} (shared: {best_ic[2]})"
    return "no keyword overlap with any IC track"

def main():
    with open(INTERCHANGE) as f:
        data = json.load(f)
    ic_tracks = [(t.get("disc_number"), t.get("track_number"), t["title"]) for t in data["tracks"]]

    with open(ROON_TITLES) as f:
        roon_titles = json.load(f)

    print(f"Roon tracks: {len(roon_titles)}  |  Interchange tracks: {len(ic_tracks)}")
    print(f"{'':3s} {'Result':6s} {'Method':8s} {'IC':6s} Roon Title / Diagnosis")
    print("-" * 120)

    matched = 0
    failed = 0
    for i, rt in enumerate(roon_titles):
        rn = normalize_title(rt)
        idx, method = find_best_match(rn, ic_tracks)
        short_rt = rt.replace("Le Nozze di Figaro: ", "")

        if idx is not None:
            d, t, _ = ic_tracks[idx]
            matched += 1
            print(f"{i+1:3d} MATCH  {method:8s} d{d}t{t:2d}  {short_rt[:85]}")
        else:
            failed += 1
            diag = diagnose_failure(rt, rn, ic_tracks)
            print(f"{i+1:3d} FAIL   {'':8s} {'':6s} {short_rt[:85]}")
            print(f"    diag: {diag}")

    print("-" * 120)
    print(f"Matched: {matched}/{len(roon_titles)}  Failed: {failed}/{len(roon_titles)}")

    # Also show which IC tracks were never matched
    matched_ic = set()
    for rt in roon_titles:
        rn = normalize_title(rt)
        idx, _ = find_best_match(rn, ic_tracks)
        if idx is not None:
            matched_ic.add(idx)
    unmatched_ic = [ic_tracks[i] for i in range(len(ic_tracks)) if i not in matched_ic]
    if unmatched_ic:
        print(f"\nInterchange tracks never matched ({len(unmatched_ic)}):")
        for d, t, title in unmatched_ic:
            print(f"  d{d}t{t:2d}  {title[:90]}")

if __name__ == "__main__":
    main()
