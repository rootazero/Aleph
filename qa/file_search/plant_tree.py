#!/usr/bin/env python3
"""Plant the trees `qa/file_search/run.sh` searches.

Two trees, because two different questions are being asked and mixing them
would make every count in the first answer depend on the fixture for the
second:

  `probe/`  — one file of each kind the walk has to treat differently:
              tracked source, a `.gitignore`d build output, a `node_modules/`
              with no `.gitignore` entry to speak for it, and a credential
              directory named by `[sandbox] deny_read_globs`. Small enough
              that every expected count is written down here, in one place, as
              a literal.

  `pages/`  — one shape, repeated past the default page size, so `offset` /
              `next_offset` have something to page over.

The needle is deliberately a value no other tree on the machine could hold. A
fixture whose marker also occurs in the repository being run from cannot tell
"the tool found my file" from "the tool found something".
"""
import json
import pathlib
import sys

NEEDLE = "QA_NEEDLE_7f3a2b"

# What `probe/` is built to contain. The driver asserts against these rather
# than recomputing them, so a change to the tree that forgets to change the
# expectation fails instead of moving the goalposts with itself.
EXPECT = {
    "needle": NEEDLE,
    # Visible with ignore rules in force: three source files, five matches.
    "visible_matches": 5,
    "visible_files": 3,
    # `no_ignore: true` lifts the gitignore AND the generated-dir floor, but
    # NOT the protected-location floor: 5 + 50 + 50, still no `.pem`.
    "no_ignore_matches": 105,
    "no_ignore_files": 5,
    # One path is withheld by the deny floor in both readings.
    "withheld": 1,
    "rs_files": 2,
    # `pages/`: forty files, four matches each.
    "page_files": 40,
    "page_matches": 160,
}


def write(path: pathlib.Path, body: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body)


def plant(root: pathlib.Path) -> None:
    probe = root / "probe"
    write(probe / ".gitignore", "generated/\n")

    write(
        probe / "src/alpha.rs",
        f"fn one() {{ /* {NEEDLE} */ }}\n"
        f"fn two() {{ let x = \"{NEEDLE}\"; }}\n"
        "fn three() {}\n"
        f"// trailing {NEEDLE}\n",
    )
    write(probe / "src/beta.rs", f"fn beta() {{}} // {NEEDLE}\n")
    write(probe / "docs/notes.md", f"A note mentioning {NEEDLE}.\n")

    # Ignored by the tree's own `.gitignore`. Fifty matches, so a result that
    # silently included it would be unmistakable rather than plausible.
    write(probe / "generated/bundle.js", "".join(f"// {NEEDLE} {i}\n" for i in range(50)))
    # NOT named in `.gitignore` — this one is only excluded because the walk
    # carries its own generated/VCS floor for trees that have no ignore file
    # to speak for them.
    write(probe / "node_modules/dep.js", "".join(f"// {NEEDLE} {i}\n" for i in range(50)))
    # The protected location. `[sandbox] deny_read_globs` names it in the
    # config this fixture boots with.
    write(probe / "secrets/qa_key.pem", f"-----BEGIN QA KEY-----\n{NEEDLE}\n")

    pages = root / "pages"
    for i in range(EXPECT["page_files"]):
        write(
            pages / f"f{i:03}.txt",
            "".join(f"line {j} {NEEDLE}\n" if j % 3 == 0 else f"line {j}\n" for j in range(12)),
        )


if __name__ == "__main__":
    root = pathlib.Path(sys.argv[1])
    plant(root)
    print(json.dumps(EXPECT))
