# -*- coding: utf-8 -*-
"""Keep the wiki pages in `docs/wiki/` from drifting off the documents they draw.

Why this exists
---------------

The GitHub wiki was empty, and so was the repository's Projects tab, and the two
look the same to somebody deciding whether this project is alive: nothing to see.
The fix is a visual page and a board. Both would restate what `docs/ROADMAP.md`
and `docs/MVP.md` already say -- which milestone is done, which acceptance test
has passed -- and a second copy of a status is what this project has paid for
every time it has had one (the test counts, the 25/75 ratio that nine files kept
announcing after it became 30/70).

So the second copy is not typed. The tables, the colour-coded milestone map, the
acceptance grid and the Now / Not started / Deferred lists in `docs/wiki/` sit
between `<!-- GENERATED:name -->` markers and are produced from the two source
tables by this file. A person edits ROADMAP.md; `--write` redraws the wiki; and
CI refuses a tree where the two disagree, which is the whole point.

The Projects board is fed from the same parse (`--items`, read by
`scripts/project-sync.sh`), so the board, the wiki and the roadmap cannot each
say something different about M7.

What is checked
---------------

* every `GENERATED` block in `docs/wiki/*.md` is exactly what the sources give;
* every milestone in the ROADMAP table has a status this file understands and a
  place on the map (`GROUPS`) -- a new milestone fails here until it is placed,
  rather than silently missing from the picture;
* every acceptance test A-O is accounted for: a row in MVP.md section 6, or an
  honest "no verdict recorded" (K, L, M and N have no row today, and that is
  worth seeing on a front page rather than hiding by omission);
* every `blob/main/<path>` link in the wiki names a file that exists, and every
  `](Page)` link names a page that exists;
* every mermaid fence is closed and non-empty;
* no test count is typed by hand outside a generated block.

What is NOT checked: that the wiki *repository* matches `docs/wiki/`. Publishing
is `scripts/wiki-publish.sh`, run with credentials this check does not have.
"""

import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
WIKI = ROOT / "docs" / "wiki"
ROADMAP = ROOT / "docs" / "ROADMAP.md"
MVP = ROOT / "docs" / "MVP.md"

REPO_URL = "https://github.com/SigSegGit/itsanas"

STATUS = {"✅": "done", "🟨": "wip", "⬜": "todo"}
# "lab" exists only for acceptance tests: MVP.md marks some tests ✅ with "in the
# laboratory" or "verified locally", and the MVP is defined as passing on the
# fleet. Painting those green would say more than section 6 does.
SQUARE = {"done": "🟩", "lab": "🟦", "wip": "🟨", "todo": "⬜"}
LABEL = {"done": "Done", "lab": "Laboratory only", "wip": "In progress", "todo": "Not started"}
# GitHub's default Status field on a new Project has exactly these three options.
PROJECT_STATUS = {"done": "Done", "lab": "In Progress", "wip": "In Progress", "todo": "Todo"}

# The map groups milestones the way ARCHITECTURE.md groups the layers. It is an
# editorial choice, not a dependency claim, which is why there are no arrows.
GROUPS = [
    ("Core: keys, store, merge", ["M1", "M1b", "M2", "M3", "M10", "M11"]),
    ("Network", ["M4", "M4b", "M5", "M6"]),
    ("What a person touches", ["M7", "M12", "M13"]),
    ("Proof", ["M0", "M8", "M9"]),
]

TESTS = "ABCDEFGHIJKLMNO"


def fail(message):
    print(f"check-wiki: {message}", file=sys.stderr)
    sys.exit(1)


def cells(line):
    return [c.strip() for c in line.strip().strip("|").split("|")]


def sort_key(key):
    m = re.fullmatch(r"M(\d+)([a-z]?)", key)
    return (int(m.group(1)), m.group(2))


def milestones():
    text = ROADMAP.read_text(encoding="utf-8")
    start = text.find("## Current state at a glance")
    if start < 0:
        fail("docs/ROADMAP.md lost its '## Current state at a glance' table")
    found = {}
    for line in text[start:].splitlines()[1:]:
        if line.startswith("## "):
            break
        m = re.match(r"^\| (M\d+[a-z]?) (.+?) \|", line)
        if not m:
            continue
        row = cells(line)
        if len(row) != 3:
            fail(f"ROADMAP row for {m.group(1)} has {len(row)} cells, expected 3 "
                 "(a literal '|' inside a cell breaks the table and this parse)")
        mark = row[2][:1]
        if mark not in STATUS:
            fail(f"ROADMAP row {m.group(1)} starts its status with {mark!r}; "
                 f"expected one of {' '.join(STATUS)}")
        key = m.group(1)
        if key in found:
            fail(f"ROADMAP lists {key} twice")
        found[key] = {
            "key": key,
            "name": m.group(2).strip(),
            "crates": row[1],
            "state": STATUS[mark],
        }
    if not found:
        fail("no milestone rows found in the ROADMAP table")
    placed = [k for _, ks in GROUPS for k in ks]
    for key in found:
        if key not in placed:
            fail(f"milestone {key} is in ROADMAP.md but on no group of the map; "
                 "add it to GROUPS in scripts/check-wiki.py")
    for key in placed:
        if key not in found:
            fail(f"GROUPS names {key}, which the ROADMAP table no longer has")
    return [found[k] for k in sorted(found, key=sort_key)]


def acceptance():
    text = MVP.read_text(encoding="utf-8")
    start = text.find("## 6. Where the MVP stands today")
    if start < 0:
        fail("docs/MVP.md lost its '## 6. Where the MVP stands today' section")
    body = text[start:]
    end = body.find("\n## ", 1)
    body = body if end < 0 else body[:end]
    names = {}
    for m in re.finditer(r"^### ([A-O])\. (.+)$", text, re.M):
        names[m.group(1)] = m.group(2).strip()
    rows = {}
    for line in body.splitlines():
        m = re.match(r"^\| ([A-O]) — (.+?) \|", line)
        if not m:
            continue
        verdict = cells(line)[1]
        if verdict[:1] not in STATUS:
            continue
        said = re.sub(r"[*_]", "", verdict[1:]).strip(" ,.")
        state = STATUS[verdict[:1]]
        if state == "done" and re.search(r"laborator|locally", said, re.I):
            state = "lab"
        rows[m.group(1)] = (state, said)
    out = []
    for letter in TESTS:
        if letter not in names:
            fail(f"MVP.md has no '### {letter}.' heading for acceptance test {letter}")
        out.append({
            "key": letter,
            "name": names[letter].rstrip("."),
            "state": rows[letter][0] if letter in rows else None,   # None = no row
            "said": rows[letter][1] if letter in rows else "",
        })
    return out


def later():
    text = ROADMAP.read_text(encoding="utf-8")
    ns = text.find("\n## Not started")
    db = text.find("\n## Deferred beyond v1")
    if ns < 0 or db < 0:
        fail("ROADMAP.md lost its 'Not started' or 'Deferred beyond v1' section")
    not_started = re.findall(r"^### (.+)$", text[ns:db], re.M)
    deferred = []
    for line in text[db + 1:].splitlines()[1:]:
        if line.startswith("## "):
            break
        m = re.match(r"^\| (?!Item|---)(.+?) \| (\S+)", line)
        if m:
            deferred.append({"name": m.group(1).strip(),
                             "state": "wip" if m.group(2) == "🟨" else "todo"})
    if not not_started or not deferred:
        fail("could not read the Not started / Deferred lists from ROADMAP.md")
    return not_started, deferred


def tally(rows):
    n = {"done": 0, "wip": 0, "todo": 0}
    for r in rows:
        n[r["state"]] += 1
    return n


def stats():
    text = ROADMAP.read_text(encoding="utf-8")
    m = re.search(r"\*\*(\d+) test functions, (\d+) of\s+them `#\[ignore\]`d into the slow job, "
                  r"and (\d+) of\s+them red-team tests", text)
    if not m:
        fail("ROADMAP.md no longer carries the 'N test functions ... M red-team' sentence")
    return int(m.group(1)), int(m.group(3))


def anchor_link(path, text):
    return f"[{text}]({REPO_URL}/blob/main/{path})"


def block_progress():
    ms = milestones()
    n = tally(ms)
    bar = "".join(SQUARE[m["state"]] for m in ms)
    return (f"{bar}\n\n"
            f"**{n['done']} of {len(ms)} milestones done**, {n['wip']} in progress, "
            f"{n['todo']} not started. One square per milestone, in order.")


def block_map():
    ms = {m["key"]: m for m in milestones()}
    lines = ["```mermaid", "flowchart LR"]
    for i, (title, keys) in enumerate(GROUPS):
        lines.append(f'  subgraph G{i}["{title}"]')
        lines.append("    direction TB")
        for k in keys:
            name = ms[k]["name"].replace('"', "'")
            lines.append(f'    {k}["{k} · {name}"]:::{ms[k]["state"]}')
        lines.append("  end")
    lines += [
        "  classDef done fill:#1a7f37,stroke:#116329,color:#ffffff",
        "  classDef wip fill:#bf8700,stroke:#7d4e00,color:#ffffff",
        "  classDef todo fill:#57606a,stroke:#32383f,color:#ffffff",
        "```",
        "",
        "Green is done, amber is in progress, grey is not started. "
        "Grouping follows the layers in "
        + anchor_link("docs/ARCHITECTURE.md", "ARCHITECTURE") + "; it says where "
        "a milestone sits, not what depends on what.",
    ]
    return "\n".join(lines)


def block_table():
    lines = ["| | Milestone | Where | Status |", "| --- | --- | --- | --- |"]
    for m in milestones():
        lines.append(f"| {SQUARE[m['state']]} | **{m['key']}** {m['name']} | "
                     f"{m['crates']} | {LABEL[m['state']]} |")
    lines += ["", "What each status means in detail, with the dates and the hardware it "
              "was watched on, is in " + anchor_link("docs/ROADMAP.md", "ROADMAP.md") + "."]
    return "\n".join(lines)


def block_acceptance():
    rows = acceptance()
    bar = " ".join(f"{r['key']}{SQUARE[r['state']] if r['state'] else '❔'}" for r in rows)
    lines = [bar, "",
             "| | Test | Verdict recorded in MVP.md §6 |", "| --- | --- | --- |"]
    for r in rows:
        if r["state"]:
            # For "lab" the phrase is what put it there; repeating it reads as a stutter.
            said = f": {r['said']}" if r["said"] and r["state"] == "done" else ""
            lines.append(f"| {SQUARE[r['state']]} | **{r['key']}** {r['name']} | "
                         f"{LABEL[r['state']]}{said} |")
        else:
            lines.append(f"| ❔ | **{r['key']}** {r['name']} | no row yet |")
    lines += ["",
              "🟦 passed in the laboratory (tests, local nodes) and has not been run on the "
              "fleet, which is what the MVP asks for. "
              "❔ is not a failure: it means MVP.md section 6 has no row for that test, so "
              "nobody has written a verdict down. That section is written by hand and can lag "
              "the nightly fleet run; where they disagree, trust the run. Passing the "
              "tests A-M and O unassisted on the fleet is the definition of the MVP in "
              + anchor_link("docs/MVP.md", "MVP.md") + "."]
    return "\n".join(lines)


def block_horizon():
    ms = milestones()
    not_started, deferred = later()
    now = [f"- 🟨 **{m['key']}** {m['name']}" for m in ms if m["state"] == "wip"]
    nxt = [f"- ⬜ **{m['key']}** {m['name']}" for m in ms if m["state"] == "todo"]
    # A heading that starts with a milestone key is that milestone, already listed.
    nxt += [f"- ⬜ {h}" for h in not_started if not re.match(r"M\d+[a-z]? ", h)]
    lat = [f"- {'🟨' if d['state'] == 'wip' else '⬜'} {d['name']}" for d in deferred]
    return "\n".join([
        "### Now: built, not finished", "", *now, "",
        "### Not started", "", *nxt, "",
        "### Deferred beyond v1", "", *lat])


def block_stats():
    tests, red = stats()
    ms = milestones()
    n = tally(ms)
    return (f"**{tests} tests, {red} of them red-team** (a red-team test passes when the "
            f"attack fails) · **{n['done']}/{len(ms)} milestones done**")


BLOCKS = {
    "stats": block_stats,
    "progress": block_progress,
    "map": block_map,
    "table": block_table,
    "acceptance": block_acceptance,
    "horizon": block_horizon,
}

MARK = re.compile(r"(<!-- GENERATED:(\w+) -->\n)(.*?)(\n<!-- /GENERATED:\2 -->)", re.S)


def render(page_text, page):
    seen = set()

    def sub(m):
        name = m.group(2)
        if name not in BLOCKS:
            fail(f"{page} has a GENERATED block {name!r} that no generator makes")
        seen.add(name)
        return m.group(1) + BLOCKS[name]() + m.group(4)

    return MARK.sub(sub, page_text), seen


def items():
    out = []
    for m in milestones():
        out.append({
            "title": f"{m['key']} — {m['name']}",
            "body": f"Milestone {m['key']}. Where: {m['crates']}.\n\n"
                    f"Status comes from docs/ROADMAP.md; edit it there, not here "
                    f"({REPO_URL}/blob/main/docs/ROADMAP.md).",
            "status": PROJECT_STATUS[m["state"]],
        })
    for r in acceptance():
        out.append({
            "title": f"MVP test {r['key']} — {r['name']}",
            "body": f"Acceptance test {r['key']}. "
                    + ("Verdict comes from docs/MVP.md section 6."
                       if r["state"] else "docs/MVP.md section 6 has no row for it yet, "
                       "so no verdict is recorded.")
                    + f"\n\n{REPO_URL}/blob/main/docs/MVP.md",
            "status": PROJECT_STATUS[r["state"] or "todo"],
        })
    _, deferred = later()
    for d in deferred:
        out.append({
            "title": f"Later — {d['name']}",
            "body": f"Deferred beyond v1. {REPO_URL}/blob/main/docs/ROADMAP.md",
            "status": PROJECT_STATUS[d["state"]],
        })
    titles = [i["title"] for i in out]
    if len(set(titles)) != len(titles):
        fail("two board items would share a title; project-sync.sh matches on title")
    return out


def check_structure(pages):
    names = {p.stem for p in pages}
    for p in pages:
        text = p.read_text(encoding="utf-8")
        rel = p.relative_to(ROOT)
        for m in re.finditer(re.escape(REPO_URL) + r"/blob/main/([^)#\s]+)", text):
            if not (ROOT / m.group(1)).exists():
                fail(f"{rel} links to {m.group(1)}, which is not in the repository")
        for m in re.finditer(r"\]\(([A-Za-z][A-Za-z0-9_-]*)\)", text):
            if m.group(1) not in names:
                fail(f"{rel} links to the wiki page {m.group(1)!r}, which has no file "
                     "in docs/wiki/")
        fences = re.findall(r"^```(\w*)\s*$", text, re.M)
        if len(fences) % 2:
            fail(f"{rel} has an unclosed code fence")
        for m in re.finditer(r"```mermaid\n(.*?)```", text, re.S):
            if not m.group(1).strip():
                fail(f"{rel} has an empty mermaid block")
        outside = MARK.sub("", text)
        m = re.search(r"\b\d[\d,.]{2,}\s+(?:tests|test functions)\b", outside)
        if m:
            fail(f"{rel} types a test count by hand ({m.group(0)!r}); "
                 "use the stats block so check-counts.py stays the only authority")


def main(argv):
    pages = sorted(p for p in WIKI.glob("*.md"))
    if not pages:
        fail("docs/wiki/ holds no pages")
    if "--items" in argv:
        json.dump(items(), sys.stdout, ensure_ascii=False, indent=2)
        print()
        return
    stale = []
    for p in pages:
        old = p.read_text(encoding="utf-8")
        new, seen = render(old, p.name)
        if new != old:
            stale.append(p)
            if "--write" in argv:
                p.write_text(new, encoding="utf-8")
    if "--write" in argv:
        print(f"check-wiki: rewrote {len(stale)} page(s)")
    elif stale:
        fail("generated blocks are out of date in "
             + ", ".join(p.name for p in stale)
             + ": ROADMAP.md or MVP.md changed and the wiki was not redrawn. "
             "Run `python scripts/check-wiki.py --write` and commit.")
    required = {"stats", "progress", "map", "table", "acceptance", "horizon"}
    present = set()
    for p in pages:
        present |= render(p.read_text(encoding="utf-8"), p.name)[1]
    missing = required - present
    if missing:
        fail("no wiki page carries the generated block(s): " + ", ".join(sorted(missing)))
    check_structure(pages)
    if "--write" not in argv:
        print(f"check-wiki: {len(pages)} pages, all generated blocks current")


if __name__ == "__main__":
    main(sys.argv[1:])
