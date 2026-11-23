# -*- coding: utf-8 -*-
"""Every language of the Android app says everything the English one says.

Why this exists
---------------
The app's native screens are `res/values/strings.xml`, and each language is a
folder beside it (`values-fr`, `values-zh-rCN`). Android shows English for any
name a language lacks, so a string added in English alone puts one English line
in the middle of a Chinese screen, and nothing fails: the release build's lint
runs only its fatal checks, and a missing translation is not one. The sync
notification was in English for French phones until 2026-11-23 for a nearby
reason: its texts were written in the Kotlin, where no translation can reach.

So this gate checks, for each language:

1. the same string names as English, no more, no fewer;
2. the same format slots (`%1$d`, `%2$s`): a lost slot shows a sentence without
   its number, a wrong type crashes the screen that formats it;
3. no bare apostrophe, which aapt refuses (`\\'` or a typographic one);

and that the languages the project ships (French, Simplified Chinese) are all
there, and that no notification, dialog or prompt text is a literal in Kotlin.
"""
import pathlib
import re
import sys
import xml.etree.ElementTree as ET

ROOT = pathlib.Path(__file__).resolve().parent.parent
RES = ROOT / "android" / "app" / "src" / "main" / "res"
KOTLIN = ROOT / "android" / "app" / "src" / "main" / "java"
SHIPPED = ["values-fr", "values-zh-rCN"]
SLOT = re.compile(r"%(\d+\$)?[-#+ 0,(]*\d*(\.\d+)?[sdfx]")
# A user-visible text set from a Kotlin literal instead of a resource.
LITERAL = re.compile(
    r'(setContentTitle|setContentText|setNegativeButtonText|setTitle|setSubtitle'
    r'|setDescription)\("[^"]*[A-Za-z]'
    r'|NotificationChannel\([^,]+,\s*"'
    r'|description\s*=\s*"[A-Za-z]'
)


def strings(path):
    texts = {}
    raw = path.read_text(encoding="utf-8")
    for element in ET.fromstring(raw).iter("string"):
        texts[element.get("name")] = "".join(element.itertext())
    for match in re.finditer(r"<string[^>]*>(.*?)</string>", raw, re.S):
        if re.search(r"(?<!\\)'", match.group(1)):
            yield_error(f"{path.relative_to(ROOT)}: a bare apostrophe, which aapt refuses: "
                        f"{match.group(1)[:60]!r}")
    return texts


ERRORS = []


def yield_error(message):
    ERRORS.append(message)


def main():
    english = strings(RES / "values" / "strings.xml")
    for folder in SHIPPED:
        if not (RES / folder / "strings.xml").exists():
            yield_error(f"{folder}/strings.xml is missing: that language would read English")
    for path in sorted(RES.glob("values-*/strings.xml")):
        folder = path.parent.name
        other = strings(path)
        missing = sorted(set(english) - set(other))
        extra = sorted(set(other) - set(english))
        if missing:
            yield_error(f"{folder}: no translation for {missing}: those lines show in English")
        if extra:
            yield_error(f"{folder}: names English no longer has: {extra}")
        for name in sorted(set(english) & set(other)):
            want = sorted(m.group(0) for m in SLOT.finditer(english[name]))
            have = sorted(m.group(0) for m in SLOT.finditer(other[name]))
            if want != have:
                yield_error(f"{folder}: {name} has slots {have}, English {want}")
    for source in sorted(KOTLIN.rglob("*.kt")):
        for number, line in enumerate(source.read_text(encoding="utf-8").splitlines(), 1):
            if LITERAL.search(line) and not line.lstrip().startswith("//"):
                yield_error(f"{source.relative_to(ROOT)}:{number}: a text in Kotlin, which no "
                            f"translation reaches; use a string resource: {line.strip()}")
    if ERRORS:
        print("\n".join(ERRORS))
        return 1
    print(f"android strings: {len(english)} names, every language complete")
    return 0


if __name__ == "__main__":
    sys.exit(main())
