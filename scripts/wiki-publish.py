# -*- coding: utf-8 -*-
"""Publish docs/wiki/ to the GitHub wiki of this repository.

    python scripts/wiki-publish.py             # clone, copy, commit, push
    python scripts/wiki-publish.py --dry-run   # show what would change, push nothing

Why Python and not a shell script: the first version was bash, and on Nicolas's
Windows machine `bash` in PowerShell is WSL, whose network was down ("Could not
resolve host: github.com"), and `python3` and `jq` are not there under Git Bash
either. Python is what the CI gates already use, and `git` is the only other
thing this needs.

Why it runs by hand: a wiki is a separate git repository
(SigSegGit/itsanas.wiki.git) that CI and cloud sessions have no credentials for,
so this runs where Nicolas is already signed in to git. The pages are kept in
docs/wiki/ so they are reviewed in a pull request and held to ROADMAP.md and
MVP.md by scripts/check-wiki.py, which this runs first: the wiki is the public
face, and a roadmap page that disagrees with ROADMAP.md is worse than none.

It never deletes a page it did not write, so anything added through the web
editor survives. A page removed from docs/wiki/ must be removed from the wiki by
hand.

GitHub creates the wiki repository when its first page is saved. If the clone
fails with "not found", open the Wiki tab, press "Create the first page", save
anything, and run this again; it overwrites that page.
"""

import os
import pathlib
import shutil
import stat
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
URL = os.environ.get("WIKI_URL", "https://github.com/SigSegGit/itsanas.wiki.git")


def git(*args, cwd=None, check=True):
    return subprocess.run(["git", *args], cwd=cwd, check=check, text=True,
                          encoding="utf-8", errors="replace")


def forget(func, path, _exc):
    # Windows marks files under .git read-only; rmtree cannot remove them as is.
    os.chmod(path, stat.S_IWRITE)
    func(path)


def main(argv):
    dry = "--dry-run" in argv
    subprocess.run([sys.executable, str(ROOT / "scripts" / "check-wiki.py")], check=True)
    src = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT, check=True,
                         capture_output=True, text=True).stdout.strip()

    work = pathlib.Path(tempfile.mkdtemp(prefix="itsanas-wiki-"))
    try:
        wiki = work / "wiki"
        if git("clone", "--quiet", URL, str(wiki), check=False).returncode:
            print(f"wiki-publish: could not clone {URL}", file=sys.stderr)
            print("  If the wiki has no page yet, create the first one in the Wiki tab "
                  "and re-run. If it says it cannot resolve github.com, the network of "
                  "the shell you ran this in is down.", file=sys.stderr)
            return 1
        for page in sorted((ROOT / "docs" / "wiki").glob("*.md")):
            shutil.copyfile(page, wiki / page.name)
        git("add", "-A", cwd=wiki)
        if git("diff", "--cached", "--quiet", cwd=wiki, check=False).returncode == 0:
            print("wiki-publish: the wiki already matches docs/wiki/")
            return 0
        git("diff", "--cached", "--stat", cwd=wiki)
        if dry:
            print("wiki-publish: dry run, nothing pushed")
            return 0
        git("commit", "--quiet", "-m", f"Sync from docs/wiki at {src}", cwd=wiki)
        git("push", "--quiet", "origin", "HEAD", cwd=wiki)
        print(f"wiki-publish: pushed ({src})")
        return 0
    finally:
        shutil.rmtree(work, onerror=forget)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
