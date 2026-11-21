#!/usr/bin/env python3
"""Positional formats that outlive one build must only ever be appended to.

bincode writes an enum variant as its POSITION and a struct as its fields in
order, with no names. A value written by one build and read by another -- an
ILM queue in LocalDB across an agent upgrade, a P2P frame between two agents of
different versions -- decodes wrongly the moment a variant is inserted, removed
or reordered, or a field is added, removed, retyped or reordered. That is the
2026-11-18 chat outage: agent 0.13.0 inserted eleven variants before
`InternalServiceRequest::Message`, and every 0.11.0 queue became unreadable
("expected variant index 0 <= i < 2"), or, holding one message, decoded
"successfully" as `IdentityStatus`.

ci/persisted-formats.txt names the ROOTS (the types such bytes are encoded
as). This walks every type reachable from each root through the source --
field types, variant fields, type aliases, in this repo and in the
intersession-layer-messaging submodule -- and compares each with its
definition at the base revision:

  * an enum's variants at the base must be a prefix of its variants now, each
    with identical fields (so a NEW variant may only be appended);
  * a struct or alias must be identical.

Serde attributes count as part of a field (`skip` removes it from the bytes).
A root whose format carries a version may change anything in a diff that also
raises its highest `<PREFIX><n>` constant: the reader then knows which layout
it holds, and keeping the old one readable is that change's job.

Usage: check-persisted-enums-are-append-only.py --base <git-rev>
The current side is the WORKING TREE, so it judges an edit before it is
committed as well as a commit in CI.
"""
from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
from dataclasses import dataclass, field

ROOT_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CONFIG = os.path.join(ROOT_DIR, "ci", "persisted-formats.txt")
SUBMODULE = "intersession-layer-messaging"
IDENT = re.compile(r"\b[A-Z][A-Za-z0-9_]*\b")
DECL = re.compile(r"\b(enum|struct|type)\s+([A-Z][A-Za-z0-9_]*)")


def git(*args: str, cwd: str = ROOT_DIR, data: bytes | None = None) -> bytes:
    run = subprocess.run(["git", *args], cwd=cwd, input=data, capture_output=True)
    if run.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)} (in {cwd}): {run.stderr.decode().strip()}")
    return run.stdout


def is_source(path: str) -> bool:
    if not path.endswith(".rs"):
        return False
    parts = path.split("/")
    if any(p in ("tests", "examples", "benches", "target") for p in parts[:-1]):
        return False
    name = parts[-1]
    return not (name == "tests.rs" or name.endswith("_tests.rs") or name.endswith("_test.rs"))


# --- reading the two revisions ------------------------------------------------


def worktree_sources() -> dict[str, str]:
    out: dict[str, str] = {}
    for repo, prefix in ((ROOT_DIR, ""), (os.path.join(ROOT_DIR, SUBMODULE), SUBMODULE + "/")):
        for path in git("ls-files", cwd=repo).decode().splitlines():
            full = os.path.join(repo, path)
            if is_source(path) and os.path.isfile(full):
                with open(full, encoding="utf-8") as f:
                    out[prefix + path] = f.read()
    return out


def blobs_at(repo: str, rev: str) -> dict[str, str]:
    paths = [p for p in git("ls-tree", "-r", "--name-only", rev, cwd=repo).decode().splitlines() if is_source(p)]
    if not paths:
        return {}
    raw = git("cat-file", "--batch", cwd=repo, data="".join(f"{rev}:{p}\n" for p in paths).encode())
    out: dict[str, str] = {}
    pos = 0
    for path in paths:
        header_end = raw.index(b"\n", pos)
        size = int(raw[pos:header_end].split()[2])
        out[path] = raw[header_end + 1 : header_end + 1 + size].decode("utf-8")
        pos = header_end + 1 + size + 1
    return out


def base_sources(base: str) -> dict[str, str]:
    out = blobs_at(ROOT_DIR, base)
    entry = git("ls-tree", base, SUBMODULE).decode().split()
    if len(entry) < 3 or entry[1] != "commit":
        raise RuntimeError(f"{SUBMODULE} is not a submodule at {base}")
    sub = os.path.join(ROOT_DIR, SUBMODULE)
    sha = entry[2]
    try:
        git("cat-file", "-e", f"{sha}^{{commit}}", cwd=sub)
    except RuntimeError:
        git("fetch", "--quiet", "origin", sha, cwd=sub)  # fails loudly if it cannot
    for path, text in blobs_at(sub, sha).items():
        out[f"{SUBMODULE}/{path}"] = text
    return out


# --- parsing --------------------------------------------------------------------


def strip(text: str) -> str:
    """Comments removed, string and char literal contents blanked."""
    out: list[str] = []
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        if text.startswith("//", i):
            j = text.find("\n", i)
            i = n if j < 0 else j
        elif text.startswith("/*", i):
            depth, i = 1, i + 2
            while i < n and depth:
                if text.startswith("/*", i):
                    depth, i = depth + 1, i + 2
                elif text.startswith("*/", i):
                    depth, i = depth - 1, i + 2
                else:
                    i += 1
        elif c == "r" and re.match(r'r#*"', text[i:]) and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")):
            hashes = re.match(r'r(#*)"', text[i:]).group(1)
            end = text.find('"' + hashes, i + 2 + len(hashes))
            out.append('""')
            i = n if end < 0 else end + 1 + len(hashes)
        elif c == '"':
            i += 1
            while i < n and text[i] != '"':
                i += 2 if text[i] == "\\" else 1
            out.append('""')
            i += 1
        elif c == "'" and re.match(r"'(\\u\{[0-9a-fA-F]+\}|\\.|[^\\'\n])'", text[i:]):
            m = re.match(r"'(\\u\{[0-9a-fA-F]+\}|\\.|[^\\'\n])'", text[i:])
            out.append("' '")
            i += m.end()
        else:
            out.append(c)
            i += 1
    return "".join(out)


OPEN, CLOSE = "([{<", ")]}>"


def balanced(text: str, start: int) -> int:
    """Index just past the bracket group opening at `start`."""
    depth, i = 0, start
    while i < len(text):
        c = text[i]
        if c == "-" and text.startswith("->", i):
            i += 2
            continue
        if c in OPEN:
            depth += 1
        elif c in CLOSE:
            depth -= 1
            if depth == 0:
                return i + 1
        i += 1
    raise ValueError("unbalanced brackets")


def split_top(body: str) -> list[str]:
    items, depth, cur, i = [], 0, [], 0
    while i < len(body):
        c = body[i]
        if c == "-" and body.startswith("->", i):
            cur.append("->")
            i += 2
            continue
        if c in OPEN:
            depth += 1
        elif c in CLOSE:
            depth -= 1
        if c == "," and depth == 0:
            items.append("".join(cur))
            cur = []
        else:
            cur.append(c)
        i += 1
    items.append("".join(cur))
    return [x.strip() for x in items if x.strip()]


def norm(s: str) -> str:
    return re.sub(r"\s+", "", s)


def take_attrs(item: str) -> tuple[list[str], str]:
    attrs, item = [], item.strip()
    while item.startswith("#"):
        end = balanced(item, item.index("["))
        attr = norm(item[item.index("[") + 1 : end - 1])
        if attr.startswith("serde("):
            attrs.append(attr)
        item = item[end:].strip()
    return attrs, item


def strip_vis(item: str) -> str:
    return re.sub(r"^pub(\s*\([^)]*\))?\s*", "", item.strip())


def fields_of(group: str) -> list[str]:
    """`{ a: T, ... }` or `( T, ... )` as a list of normalized fields."""
    out = []
    for item in split_top(group[1:-1]):
        attrs, rest = take_attrs(item)
        out.append(";".join(attrs + [norm(strip_vis(rest))]))
    return out


@dataclass
class TypeDef:
    kind: str  # enum | struct | type
    name: str
    path: str
    items: list[tuple[str, list[str]]] = field(default_factory=list)  # enum: (variant, fields)
    fields: list[str] = field(default_factory=list)  # struct / alias body

    def references(self) -> set[str]:
        texts = self.fields + [f for _, fs in self.items for f in fs]
        return {m for t in texts for m in IDENT.findall(t)}

    def describe(self) -> str:
        if self.kind == "enum":
            return ", ".join(v + (f"{{{', '.join(fs)}}}" if fs else "") for v, fs in self.items)
        return ", ".join(self.fields)


def parse(path: str, text: str) -> list[TypeDef]:
    src = strip(text)
    defs = []
    for m in DECL.finditer(src):
        try:
            parse_one(src, m, path, defs)
        except ValueError as err:
            # Macro-generated or otherwise unusual: harmless unless a root reaches it.
            defs.append(TypeDef("unreadable", m.group(2), path, fields=[str(err)]))
    return defs


def parse_one(src: str, m: re.Match, path: str, defs: list[TypeDef]) -> None:
    kind, name = m.group(1), m.group(2)
    i = m.end()
    while i < len(src) and src[i].isspace():
        i += 1
    if i < len(src) and src[i] == "<":
        i = balanced(src, i)
    rest = src[i:]
    if kind == "type":
        eq = re.match(r"\s*=", rest)
        if not eq:
            return  # an associated type declaration, not an alias
        j, depth = i + eq.end(), 0
        while j < len(src) and not (src[j] == ";" and depth == 0):
            depth += src[j] in "([{<"
            depth -= src[j] in ")]}>"
            j += 1
        defs.append(TypeDef("type", name, path, fields=[norm(src[i + eq.end() : j])]))
        return
    where = re.match(r"\s*(where\b[^{;(]*)?", rest)
    j = i + where.end()
    if j >= len(src) or src[j] not in "{(":
        if kind == "struct":
            defs.append(TypeDef("struct", name, path))  # unit struct
        return
    end = balanced(src, j)
    group = src[j:end]
    if kind == "struct":
        defs.append(TypeDef("struct", name, path, fields=fields_of(group)))
        return
    items = []
    for item in split_top(group[1:-1]):
        attrs, rest_item = take_attrs(item)
        vm = re.match(r"([A-Za-z_][A-Za-z0-9_]*)\s*", rest_item)
        if not vm:
            raise ValueError(f"{path}: cannot read a variant of {name}: {item!r}")
        vname, tail = vm.group(1), rest_item[vm.end() :]
        grouped = tail[:1] in ("{", "(")
        vfields = fields_of(tail[: balanced(tail, 0)]) if grouped else []
        disc = re.search(r"=\s*(.+)$", tail)
        if disc and not grouped:
            vfields = [f"={norm(disc.group(1))}"]
        items.append((";".join(attrs + [vname]), vfields))
    defs.append(TypeDef("enum", name, path, items=items))


def index(sources: dict[str, str]) -> dict[str, list[TypeDef]]:
    out: dict[str, list[TypeDef]] = {}
    for path, text in sources.items():
        for d in parse(path, text):
            out.setdefault(d.name, []).append(d)
    return out


# --- the check --------------------------------------------------------------------


@dataclass
class Root:
    name: str
    path: str
    version_prefix: str | None


def read_config() -> tuple[list[Root], set[str]]:
    roots, opaque = [], set()
    with open(CONFIG, encoding="utf-8") as f:
        for n, line in enumerate(f, 1):
            line = line.split("#", 1)[0].strip()
            if not line:
                continue
            words = line.split()
            if words[0] == "root" and len(words) == 4:
                roots.append(Root(words[1], words[2], None if words[3] == "-" else words[3]))
            elif words[0] == "opaque" and len(words) == 2:
                opaque.add(words[1])
            else:
                raise RuntimeError(f"{CONFIG}:{n}: expected `root <Type> <file> <PREFIX|->` or `opaque <Type>`")
    if not roots:
        raise RuntimeError(f"{CONFIG} names no roots, so this check would compare nothing")
    return roots, opaque


def reachable(root: Root, types: dict[str, list[TypeDef]], opaque: set[str]) -> dict[tuple[str, str], TypeDef]:
    start = [d for d in types.get(root.name, []) if d.path == root.path]
    if not start:
        raise RuntimeError(
            f"root {root.name} is not defined in {root.path}; a renamed or moved root checks nothing, "
            f"so update ci/persisted-formats.txt"
        )
    seen: dict[tuple[str, str], TypeDef] = {}
    todo = list(start)
    while todo:
        d = todo.pop()
        if (d.name, d.path) in seen:
            continue
        seen[(d.name, d.path)] = d
        for ref in d.references():
            if ref not in opaque:
                todo.extend(types.get(ref, []))
    return seen


def highest_version(text: str | None, prefix: str) -> int:
    if text is None:
        return -1
    found = [int(v) for v in re.findall(rf"\bconst\s+{re.escape(prefix)}(\d+)\b", text)]
    return max(found, default=-1)


def compare(old: TypeDef, new: TypeDef) -> str | None:
    if "unreadable" in (old.kind, new.kind):
        return f"this check cannot read its definition ({(new if new.kind == 'unreadable' else old).fields[0]})"
    if old.kind != new.kind:
        return f"was {old.kind}, now {new.kind}"
    if old.kind != "enum":
        return None if old.fields == new.fields else "fields changed (bincode encodes every field, in order)"
    if new.items[: len(old.items)] == old.items:
        return None
    for i, (o, n) in enumerate(zip(old.items, new.items)):
        if o != n:
            if o[0] != n[0]:
                return f"variant {i} was {o[0]}, now {n[0]} (only appending keeps every older tag)"
            return f"variant {o[0]}'s fields changed"
    return f"{len(old.items) - len(new.items)} trailing variant(s) removed"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--base", required=True, help="the revision whose formats are already deployed")
    parser.add_argument("--list", action="store_true", help="print every type each root reaches")
    args = parser.parse_args()

    roots, opaque = read_config()
    now_src, base_src = worktree_sources(), base_sources(args.base)
    now_types, base_types = index(now_src), index(base_src)
    failures = []
    for root in roots:
        reached = reachable(root, now_types, opaque)
        if root.version_prefix and highest_version(now_src.get(root.path), root.version_prefix) > highest_version(
            base_src.get(root.path), root.version_prefix
        ):
            print(f"{root.name}: format version raised ({root.version_prefix}*); its layout may change in this diff")
            continue
        for (name, path), new in sorted(reached.items()):
            if args.list:
                print(f"  {root.name} reaches {name} ({path})")
            before = [d for d in base_types.get(name, []) if d.path == path] or (
                base_types.get(name, []) if len(base_types.get(name, [])) == 1 else []
            )
            if not before:
                continue  # new in this diff: nothing deployed to stay compatible with
            why = compare(before[0], new)
            if why:
                failures.append((root, new, before[0], why))
        print(f"{root.name}: {len(reached)} reachable type(s) checked against {args.base}")

    if not failures:
        print("OK: every persisted positional format only appended.")
        return 0
    for root, new, old, why in failures:
        print(f"::error file={new.path}::{new.kind} {new.name}, reachable from {root.name}: {why}")
        print(f"    before: {old.describe()}")
        print(f"    now:    {new.describe()}")
    print(
        "\nFAIL: bytes written by the deployed build would decode wrongly or not at all.\n"
        "Append the new variant at the END instead of inserting it, leave existing fields as they are,\n"
        "or, for a root with a version prefix, add the next format constant and keep the old layout readable.\n"
        "If a path to this type is never encoded on this root, mark it `opaque` in ci/persisted-formats.txt\n"
        "with the code that proves it."
    )
    return 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except RuntimeError as err:
        print(f"FAIL: {err}")
        sys.exit(1)
