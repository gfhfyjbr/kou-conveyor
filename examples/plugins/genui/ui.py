#!/usr/bin/env python3
"""The UI tool of the genui plugin: it shows a surface, a small native UI of
components bound to data and to workspace files, in the browser cockpit, the
terminal and the iPhone app.

This is a prototype, as a plugin: the tool keeps each
version of a surface in .harness/ui/<session>/<surface>/ (v<N>.json, and
latest.json for the newest), and the plugin's web script, the terminal and
the app draw the version a call made. The files a surface names as sources
are read live by the client that draws it; a version also keeps them as they
were, so older versions show a snapshot.

The arguments come as one JSON object on standard input. What this prints
is the tool's result: on its first line the version and where it is kept
(the web script finds the surface by it), then warnings, then a text render
of the surface: what the model can check without a screenshot. Errors say
where they are, and exit 2 without writing anything.
"""

import csv
import fcntl
import io
import json
import os
import re
import subprocess
import sys
import tempfile
from datetime import datetime, timezone

FORMAT = "genui/v0"
SURFACE_ID = re.compile(r"^[a-z0-9][a-z0-9_-]{0,47}$")
COMPONENT_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}$")
SOURCE_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_-]{0,47}$")
# The first line of a result: what the web script and a forked session look for.
MARKER = re.compile(r"UI ([a-z0-9][a-z0-9_-]*) v(\d+)( deleted)? \((\.harness/ui/[A-Za-z0-9_./-]+?\.json)\)")
TEMPLATE = re.compile(r"\{(/[^{}]*|#[A-Za-z0-9][A-Za-z0-9_.-]*)\}")

MAX_PARSE = 5 << 20        # bytes of a source read to render it
MAX_SNAPSHOT = 256 << 10   # bytes of sources a version keeps
MAX_ROWS = 10000           # rows of a jsonl or csv source
MAX_TEXT = 64 << 10        # characters of a text source
MAX_HTML = 512000          # characters of an Html component
MAX_PATCH = 64 << 10       # characters of a Diff
WIDTH = 80

FORMATS = ("json", "jsonl", "csv", "tsv", "text", "markdown")
TONES = ("ok", "warn", "err", "info", "neutral")

# The catalog: each component's properties by kind, and the ones it needs.
CATALOG = {
    "Column": ({"children": "ids", "gap": "enum:none|small|large"}, ["children"]),
    "Row": ({"children": "ids", "gap": "enum:none|small|large", "align": "enum:start|center|end|stretch"}, ["children"]),
    "Grid": ({"children": "ids", "columns": "int"}, ["children"]),
    "Section": ({"title": "text", "children": "ids"}, ["children"]),
    "Tabs": ({"tabs": "tabs"}, ["tabs"]),
    "Disclosure": ({"label": "text", "children": "ids", "open": "bool"}, ["label", "children"]),
    "Text": ({"text": "text", "style": "enum:body|heading|caption|mono"}, ["text"]),
    "Markdown": ({"text": "text"}, ["text"]),
    "Metric": ({"label": "text", "value": "value", "unit": "str", "previous": "value",
                "better": "enum:higher|lower", "digits": "int"}, ["value"]),
    "Badge": ({"text": "text", "tone": "tone"}, ["text"]),
    "Progress": ({"label": "text", "value": "value", "max": "value"}, ["value"]),
    "Table": ({"rows": "list", "columns": "columns", "limit": "int", "reverse": "bool"}, ["rows"]),
    "List": ({"items": "list", "text": "str", "limit": "int", "reverse": "bool"}, ["items"]),
    "Chart": ({"kind": "enum:line|bar|points", "data": "list", "x": "str", "y": "fields", "alt": "str",
               "height": "int", "unit": "str", "limit": "int"}, ["data", "y", "alt"]),
    "Image": ({"file": "file", "alt": "str"}, ["file", "alt"]),
    "Code": ({"file": "file", "text": "text", "lines": "str", "language": "str"}, []),
    "Diff": ({"file": "file", "against": "str", "patch": "str"}, []),
    "FileLink": ({"file": "file", "label": "str", "line": "int"}, ["file"]),
    "Button": ({"label": "text", "action": "action", "style": "enum:default|primary|danger", "once": "bool",
                "disabled": "value"}, ["label", "action"]),
    "TextField": ({"label": "text", "value": "bind", "placeholder": "str", "multiline": "bool"}, ["value"]),
    "Toggle": ({"label": "text", "value": "bind"}, ["value"]),
    "Select": ({"label": "text", "value": "bind", "options": "options"}, ["value", "options"]),
    "Slider": ({"label": "text", "value": "bind", "min": "number", "max": "number", "step": "number"}, ["value"]),
    "Html": ({"html": "str", "file": "file", "alt": "str", "height": "int", "inputs": "pointers",
              "actions": "names"}, ["alt"]),
}
# What shows a value, and so is worth a warning when its path finds nothing.
SHOWN = {"Metric": "value", "Progress": "value", "Table": "rows", "List": "items", "Chart": "data",
         "Text": "text", "Markdown": "text", "Badge": "text"}

MISSING = object()


class Invalid(Exception):
    pass


# ---------------------------------------------------------------- JSON Pointer

def tokens_of(pointer):
    if pointer == "":
        return []
    if not isinstance(pointer, str) or not pointer.startswith("/"):
        raise Invalid("a path is a JSON Pointer, which starts with /")
    return [t.replace("~1", "/").replace("~0", "~") for t in pointer[1:].split("/")]


def pointer_get(doc, pointer):
    try:
        tokens = tokens_of(pointer)
    except Invalid:
        return MISSING
    current = doc
    for token in tokens:
        if isinstance(current, dict):
            if token not in current:
                return MISSING
            current = current[token]
        elif isinstance(current, list):
            try:
                i = int(token)
            except ValueError:
                return MISSING
            if i < 0:  # an extension: -1 is the last element, -2 the one before
                i += len(current)
            if not 0 <= i < len(current):
                return MISSING
            current = current[i]
        else:
            return MISSING
    return current


def pointer_set(doc, pointer, value):
    """Sets (or with None removes) what pointer names in doc, an object,
    making the objects on the way; returns the document."""
    tokens = tokens_of(pointer)
    if not tokens:
        if value is None:
            return {}
        if not isinstance(value, dict):
            raise Invalid("the data model is an object")
        return value
    current = doc
    for token in tokens[:-1]:
        if isinstance(current, list):
            i = int(token) if re.fullmatch(r"\d+", token) else -1
            if not 0 <= i < len(current):
                raise Invalid(f"no element {token} on the way")
            current = current[i]
            continue
        following = current.get(token)
        if not isinstance(following, (dict, list)):
            following = {}
            current[token] = following
        current = following
    last = tokens[-1]
    if isinstance(current, list):
        if last == "-" and value is not None:
            current.append(value)
            return doc
        i = int(last) if re.fullmatch(r"\d+", last) else -1
        if not 0 <= i < len(current):
            raise Invalid(f"no element {last}")
        if value is None:
            current.pop(i)
        else:
            current[i] = value
        return doc
    if value is None:
        current.pop(last, None)
    else:
        current[last] = value
    return doc


def is_binding(value):
    return isinstance(value, dict) and set(value) == {"path"} and isinstance(value["path"], str)


# ---------------------------------------------------------------- sources

def infer_format(path):
    ext = os.path.splitext(path)[1].lower()
    return {".json": "json", ".jsonl": "jsonl", ".ndjson": "jsonl", ".csv": "csv", ".tsv": "tsv",
            ".md": "markdown", ".markdown": "markdown"}.get(ext, "text")


NUMBER = re.compile(r"-?(\d+\.?\d*|\.\d+)([eE][-+]?\d+)?")


def cell(text):
    t = text.strip()
    if NUMBER.fullmatch(t):
        try:
            return int(t) if re.fullmatch(r"-?\d+", t) else float(t)
        except ValueError:
            pass
    return text


def parse_source(fmt, text):
    """What a source's text is in the data model, as the web script makes it."""
    if fmt == "json":
        return json.loads(text)
    if fmt == "jsonl":
        rows = []
        for line in text.splitlines():
            line = line.strip()
            if not line:
                continue
            try:
                rows.append(json.loads(line))
            except ValueError:
                continue  # a line being written, or a stray one
        count = len(rows)
        rows = rows[-MAX_ROWS:]
        return {"rows": rows, "last": rows[-1] if rows else None, "count": count}
    if fmt in ("csv", "tsv"):
        table = [r for r in csv.reader(io.StringIO(text), delimiter="," if fmt == "csv" else "\t")]
        if not table:
            return {"columns": [], "rows": [], "last": None, "count": 0}
        columns = [c.strip() for c in table[0]]
        rows = []
        for r in table[1:]:
            if not any(c.strip() for c in r):
                continue
            rows.append({(columns[i] if i < len(columns) and columns[i] else f"col{i + 1}"): cell(c)
                         for i, c in enumerate(r)})
        count = len(rows)
        rows = rows[-MAX_ROWS:]
        return {"columns": columns, "rows": rows, "last": rows[-1] if rows else None, "count": count}
    return text[:MAX_TEXT]


def inside(root, path):
    return path == root or path.startswith(root + os.sep)


def workspace_path(ws, value):
    """A path of the workspace, from the root, or Invalid."""
    if not isinstance(value, str) or not value.strip():
        raise Invalid("a file is a path in the workspace")
    raw = value.strip()
    if raw.startswith("$"):  # as in prompts: $cmd/main.go
        raw = raw[1:]
    full = os.path.normpath(raw if os.path.isabs(raw) else os.path.join(ws, raw))
    if not inside(ws, full):
        raise Invalid(f"{value} is outside the workspace")
    relative = os.path.relpath(full, ws)
    parts = relative.split(os.sep)
    if parts[0] == ".git" or any(p == ".env" or p.startswith(".env.") for p in parts):
        raise Invalid(f"{relative}: a surface does not show .git or .env files")
    return relative.replace(os.sep, "/")


def iso(ts):
    return datetime.fromtimestamp(ts, timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def read_source(ws, decl):
    """The source's value (or MISSING) and its sourceInfo."""
    relative = decl["file"]
    info = {"path": relative, "format": decl["format"]}
    full = os.path.join(ws, relative)
    real = os.path.realpath(full)
    if not inside(os.path.realpath(ws), real):
        info["error"] = "the file leads outside the workspace"
        return MISSING, info
    try:
        st = os.stat(real)
    except OSError:
        info["error"] = "no such file"
        return MISSING, info
    info["size"], info["mtime"] = st.st_size, iso(st.st_mtime)
    if os.path.isdir(real):
        info["error"] = "a folder, not a file"
        return MISSING, info
    with open(real, "rb") as f:
        data = f.read(MAX_PARSE + 1)
    if len(data) > MAX_PARSE:
        if decl["format"] in ("jsonl", "csv", "tsv"):
            # The newest rows are at the end: keep those, from a whole line.
            with open(real, "rb") as f:
                f.seek(max(0, st.st_size - MAX_PARSE))
                data = f.read()
            data = data[data.find(b"\n") + 1:]
            info["cut"] = True
        else:
            info["error"] = f"bigger than {MAX_PARSE >> 20} MB"
            return MISSING, info
    text = data.decode("utf-8", errors="replace")
    try:
        return parse_source(decl["format"], text), info
    except ValueError as e:
        info["error"] = f"not {decl['format']}: {e}"
        return MISSING, info


def trimmed(values, infos):
    """The sources as a version keeps them: at most MAX_SNAPSHOT of JSON,
    the newest rows kept."""
    out = dict(values)
    size = lambda v: len(json.dumps(v, ensure_ascii=False))
    total = sum(size(v) for v in out.values())
    for name in sorted(out, key=lambda n: -size(out[n])):
        if total <= MAX_SNAPSHOT:
            break
        value = out[name]
        before = size(value)
        if isinstance(value, dict) and isinstance(value.get("rows"), list):
            rows = list(value["rows"])
            while rows and size({**value, "rows": rows}) > max(1024, before - (total - MAX_SNAPSHOT)):
                rows = rows[len(rows) // 4 + 1:]
            out[name] = {**value, "rows": rows}
        elif isinstance(value, str):
            out[name] = value[: max(1024, len(value) - (total - MAX_SNAPSHOT))]
        else:
            out[name] = None
        infos[name]["snapshot_cut"] = True
        total += size(out[name]) - before
    return out


# ---------------------------------------------------------------- checks

class Checker:
    def __init__(self, ws, ids, metrics, sources):
        self.ws, self.ids, self.metrics, self.sources = ws, ids, metrics, sources
        self.errors, self.warnings = [], []

    def error(self, where, text):
        self.errors.append(f"{where}: {text}")

    def warn(self, where, text):
        self.warnings.append(f"{where}: {text}")

    def path(self, where, pointer, writes=False):
        try:
            tokens = tokens_of(pointer)
        except Invalid as e:
            return self.error(where, str(e))
        if tokens and tokens[0] in ("sources", "sourceInfo"):
            if writes:
                return self.error(where, f"{pointer} is a file's; an input writes to the data model, as /comment")
            if len(tokens) < 2 or tokens[1] not in self.sources:
                name = tokens[1] if len(tokens) > 1 else ""
                return self.error(where, f"the path {pointer} names no declared source" + (f' "{name}"' if name else ""))

    def template(self, where, text):
        for m in TEMPLATE.finditer(text):
            ref = m.group(1)
            if ref.startswith("#"):
                if ref[1:] not in self.metrics:
                    self.error(where, f"{{{ref}}} names no Metric")
            else:
                self.path(where, ref)

    def value(self, where, value, kind):
        if kind == "ids":
            if not isinstance(value, list):
                return self.error(where, "a list of component IDs")
            for j, child in enumerate(value):
                if not isinstance(child, str) or child not in self.ids:
                    self.error(f"{where}[{j}]", f'no component "{child}"')
            return
        if kind == "tabs":
            if not isinstance(value, list) or not value:
                return self.error(where, 'a list of { "label", "child" }')
            for j, tab in enumerate(value):
                if not isinstance(tab, dict) or not isinstance(tab.get("label"), str):
                    self.error(f"{where}[{j}]", 'a tab is { "label": "…", "child": "<id>" }')
                elif tab.get("child") not in self.ids:
                    self.error(f"{where}[{j}].child", f'no component "{tab.get("child")}"')
            return
        if kind == "bind":
            if not is_binding(value):
                return self.error(where, 'an input is bound to the data model: { "path": "/name" }')
            return self.path(where, value["path"], writes=True)
        if is_binding(value):
            if kind in ("str", "int", "bool", "file", "fields", "columns", "options", "action", "pointers", "names") \
                    or kind.startswith("enum:"):
                return self.error(where, "takes a value here, not a path")
            return self.path(where, value["path"])
        if kind == "text":
            if not isinstance(value, (str, int, float)):
                return self.error(where, 'text, or { "path": "/…" }')
            if isinstance(value, str):
                self.template(where, value)
        elif kind == "str":
            if not isinstance(value, str):
                self.error(where, "text")
        elif kind == "int":
            if not isinstance(value, int) or isinstance(value, bool):
                self.error(where, "a whole number")
        elif kind == "bool":
            if not isinstance(value, bool):
                self.error(where, "true or false")
        elif kind == "number":
            if not isinstance(value, (int, float)) or isinstance(value, bool):
                self.error(where, "a number")
        elif kind == "list":
            if not isinstance(value, list):
                self.error(where, 'a list, or { "path": "/…" } of one')
        elif kind == "tone":
            if value not in TONES:
                self.error(where, "one of " + ", ".join(TONES))
        elif kind == "fields":
            fields = [value] if isinstance(value, str) else value
            if not isinstance(fields, list) or not fields or len(fields) > 6 or not all(isinstance(f, str) for f in fields):
                self.error(where, "a field, or a list of up to 6")
        elif kind == "columns":
            if not isinstance(value, list):
                return self.error(where, 'a list of fields, or of { "key", "label", "align" }')
            for j, col in enumerate(value):
                if not (isinstance(col, str) or isinstance(col, dict) and isinstance(col.get("key"), str)):
                    self.error(f"{where}[{j}]", 'a field, or { "key": "…", "label": "…" }')
        elif kind == "options":
            if not isinstance(value, list) or not value:
                return self.error(where, 'a list of values, or of { "value", "label" }')
            for j, opt in enumerate(value):
                if not (isinstance(opt, (str, int, float)) or isinstance(opt, dict) and "value" in opt):
                    self.error(f"{where}[{j}]", 'a value, or { "value": …, "label": "…" }')
        elif kind == "file":
            try:
                return workspace_path(self.ws, value)
            except Invalid as e:
                self.error(where, str(e))
        elif kind == "action":
            self.action(where, value)
        elif kind == "pointers":
            if not isinstance(value, list):
                return self.error(where, 'a list of paths into the data model, as ["/sources/runs", "/filter"]')
            for j, pointer in enumerate(value):
                if not isinstance(pointer, str) or not pointer.startswith("/"):
                    self.error(f"{where}[{j}]", 'a path into the data model, as "/filter"')
                else:
                    self.path(f"{where}[{j}]", pointer)
        elif kind == "names":
            if not isinstance(value, list) or not all(isinstance(n, str) and n.strip() for n in value):
                self.error(where, 'a list of event names, as ["explain"]')
        elif kind.startswith("enum:"):
            allowed = kind[5:].split("|")
            if value not in allowed:
                self.error(where, "one of " + ", ".join(allowed))

    def action(self, where, action):
        kinds = ("event", "set", "open", "copy")
        if not isinstance(action, dict) or len([k for k in action if k in kinds]) != 1:
            return self.error(where, 'one of { "event": { "name": "…" } }, { "set": { "/path": value } }, '
                                     '{ "open": "file" }, { "copy": "text" }')
        if "event" in action:
            event = action["event"]
            if not isinstance(event, dict) or not isinstance(event.get("name"), str) or not event["name"].strip():
                return self.error(f"{where}.event", '{ "name": "apply", "data": { … } }')
            data = event.get("data")
            if data is not None and not isinstance(data, dict):
                self.error(f"{where}.event.data", "an object; its values may be paths")
            for key, value in (data or {}).items():
                if is_binding(value):
                    self.path(f"{where}.event.data.{key}", value["path"])
        elif "set" in action:
            if not isinstance(action["set"], dict) or not action["set"]:
                return self.error(f"{where}.set", '{ "/path": value }')
            for pointer in action["set"]:
                self.path(f"{where}.set", pointer, writes=True)
        elif "open" in action:
            try:
                action["open"] = workspace_path(self.ws, action["open"])
            except Invalid as e:
                self.error(f"{where}.open", str(e))
        elif not isinstance(action["copy"], str) and not is_binding(action["copy"]):
            self.error(f"{where}.copy", 'text, or { "path": "/…" }')

    def component(self, where, c):
        name = c.get("component")
        if name not in CATALOG:
            self.warn(f"{where}.component", f'"{name}" is not in the catalog; clients show its "alt" in its place')
            return
        props, required = CATALOG[name]
        for prop in required:
            if prop not in c:
                self.error(where, f'{name} needs "{prop}"')
        for prop, kind in props.items():
            if prop in c:
                fixed = self.value(f"{where}.{prop}", c[prop], kind)
                if kind == "file" and fixed:
                    c[prop] = fixed
        if name == "Code" and "file" not in c and "text" not in c:
            self.error(where, 'Code needs "file" or "text"')
        if name == "Diff" and "file" not in c and "patch" not in c:
            self.error(where, 'Diff needs "file" or "patch"')
        if name == "Html":
            if ("html" in c) == ("file" in c):
                self.error(where, 'Html needs "html" or "file", one of them')
            elif isinstance(c.get("html"), str) and not c["html"].strip():
                self.error(f"{where}.html", "an empty page")
            elif isinstance(c.get("file"), str) and os.path.splitext(c["file"])[1].lower() not in (".html", ".htm"):
                self.error(f"{where}.file", "an Html's file is a .html or .htm file")
            if isinstance(c.get("html"), str) and len(c["html"]) > MAX_HTML:
                self.error(f"{where}.html", f"longer than {MAX_HTML} characters")


# ---------------------------------------------------------------- the text render

def num(value, digits=None):
    """A number as a surface shows it: digits fixed, or else as few as say
    it (8.5, 41.6, 0.12), and thousands apart (1 204), as the web does."""
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return str(value)
    if value != value or value in (float("inf"), float("-inf")):
        return str(value)
    fixed = digits is not None
    if not fixed:
        a = abs(value)
        digits = 0 if float(value).is_integer() or a >= 100 else 1 if a >= 10 else 2
    text = f"{abs(value):,.{digits}f}".replace(",", " ")
    if not fixed and "." in text:
        text = text.rstrip("0").rstrip(".")
    return ("-" if value < 0 else "") + text


def as_number(value):
    if isinstance(value, bool):
        return None
    if isinstance(value, (int, float)):
        return value
    if isinstance(value, str) and NUMBER.fullmatch(value.strip()):
        return float(value)
    return None


def delta(value, previous):
    v, p = as_number(value), as_number(previous)
    if v is None or p is None or p == 0:
        return ""
    pct = (v - p) / abs(p) * 100
    if abs(pct) < 0.05:
        return "=0%"
    shown = f"{abs(pct):.1f}" if abs(pct) < 1 else f"{abs(pct):.0f}"
    return ("▲" if pct > 0 else "▼") + shown + "%"


def previous_of(doc, c):
    """A Metric's previous value: its own, or the row before the last of the
    source its value is the last row of."""
    if "previous" in c:
        return resolve(doc, c["previous"])
    value = c.get("value")
    if is_binding(value):
        m = re.fullmatch(r"/sources/([^/]+)/last/(.+)", value["path"])
        if m:
            return pointer_get(doc, f"/sources/{m.group(1)}/rows/-2/{m.group(2)}")
    return MISSING


def resolve(doc, value):
    if is_binding(value):
        return pointer_get(doc, value["path"])
    return value


def shown(value):
    if value is MISSING or value is None:
        return "—"
    if isinstance(value, bool):  # as JSON says it, and the page
        return "true" if value else "false"
    if isinstance(value, (dict, list)):
        return json.dumps(value, ensure_ascii=False)
    return num(value) if isinstance(value, (int, float)) and not isinstance(value, bool) else str(value)


def metric_text(doc, c):
    value = resolve(doc, c.get("value"))
    unit = c.get("unit", "")
    text = (num(value, c.get("digits")) if as_number(value) is not None and not isinstance(value, str)
            else shown(value)) + (f" {unit}" if unit and value is not MISSING and value is not None else "")
    change = delta(value, previous_of(doc, c))
    return text + (f" {change}" if change else "")


def fill(doc, comps, template):
    def one(m):
        ref = m.group(1)
        if ref.startswith("#"):
            c = comps.get(ref[1:])
            return metric_text(doc, c) if c else "?"
        return shown(pointer_get(doc, ref))
    return TEMPLATE.sub(one, template)


SPARK = "▁▂▃▄▅▆▇█"


def spark(values):
    values = [v for v in values if v is not None][-40:]
    if not values:
        return ""
    lo, hi = min(values), max(values)
    if hi == lo:
        return SPARK[3] * len(values)
    return "".join(SPARK[min(7, int((v - lo) / (hi - lo) * 7.999))] for v in values)


def cut(text, width):
    text = str(text).replace("\n", " ")
    return text if len(text) <= width else text[: max(0, width - 1)] + "…"


def wrap(text, width):
    out = []
    for para in str(text).splitlines() or [""]:
        line = ""
        for word in para.split(" "):
            if line and len(line) + 1 + len(word) > width:
                out.append(line)
                line = word
            else:
                line = f"{line} {word}" if line else word
        out.append(line)
    return out


class TextRender:
    INLINE = {"Metric", "Badge", "Button", "Toggle", "FileLink", "Select", "Slider"}

    def __init__(self, state, doc, extras):
        self.comps = {c["id"]: c for c in state["components"]}
        self.doc, self.extras = doc, extras

    def val(self, value):
        return resolve(self.doc, value)

    def text_of(self, value):
        value = self.val(value)
        if value is MISSING:
            return "—"
        return fill(self.doc, self.comps, value) if isinstance(value, str) else shown(value)

    def inline(self, c):
        kind = c.get("component")
        label = self.text_of(c["label"]) if "label" in c else ""
        if kind == "Metric":
            return (label.upper() + " " if label else "") + metric_text(self.doc, c)
        if kind == "Badge":
            return f"[{self.text_of(c.get('text'))}]"
        if kind == "Button":
            used = " (once)" if c.get("once") else ""
            return f"[ {label} ]{used}"
        if kind == "Toggle":
            return ("[x] " if self.val(c.get("value")) is True else "[ ] ") + label
        if kind == "FileLink":
            line = f":{c['line']}" if c.get("line") else ""
            return f"↗ {c.get('label') or c.get('file')}{line}"
        if kind == "Select":
            return f"{label}: ({shown(self.val(c.get('value')))}) ▾"
        if kind == "Slider":
            return f"{label}: {shown(self.val(c.get('value')))} [{c.get('min', 0)}…{c.get('max', 100)}]"
        return ""

    def lines(self, cid, width, depth=0):
        c = self.comps.get(cid)
        if c is None or depth > 32:
            return []
        kind = c.get("component")
        label = self.text_of(c["label"]) if "label" in c else ""
        if kind in self.INLINE:
            return [cut(self.inline(c), width)]
        if kind in ("Column", "Grid"):
            out = []
            for child in c.get("children", []):
                out += self.lines(child, width, depth + 1)
            return out
        if kind == "Row":
            kids = [self.comps.get(k) for k in c.get("children", [])]
            if kids and all(k and k.get("component") in self.INLINE for k in kids):
                out, line = [], ""
                for k in kids:
                    piece = self.inline(k)
                    if line and len(line) + 3 + len(piece) > width:
                        out.append(line)
                        line = piece
                    else:
                        line = f"{line}   {piece}" if line else piece
                return out + [cut(line, width)]
            out = []
            for child in c.get("children", []):
                out += self.lines(child, width, depth + 1)
            return out
        if kind == "Section":
            title = self.text_of(c.get("title", "")).upper()
            out = [cut(f"── {title} " + "─" * max(0, width - len(title) - 4), width)]
            for child in c.get("children", []):
                out += ["  " + line for line in self.lines(child, width - 2, depth + 1)]
            return out
        if kind == "Tabs":
            tabs = c.get("tabs", [])
            heads = "  ".join((f"[{t.get('label')}]" if i == 0 else t.get("label", "")) for i, t in enumerate(tabs))
            return [cut(heads, width)] + (self.lines(tabs[0].get("child"), width, depth + 1) if tabs else [])
        if kind == "Disclosure":
            if c.get("open"):
                out = [cut(f"▾ {label}", width)]
                for child in c.get("children", []):
                    out += ["  " + line for line in self.lines(child, width - 2, depth + 1)]
                return out
            return [cut(f"▸ {label}", width)]
        if kind == "Text":
            text = self.text_of(c.get("text"))
            return wrap(text.upper() if c.get("style") == "heading" else text, width)
        if kind == "Markdown":
            text = self.text_of(c.get("text"))
            lines = text.splitlines()
            return [cut(line, width) for line in lines[:12]] + ([f"… {len(lines) - 12} more lines"] if len(lines) > 12 else [])
        if kind == "Progress":
            value, top = as_number(self.val(c.get("value"))), as_number(self.val(c.get("max", 1))) or 1
            if value is None:
                return [cut(f"{label} —", width)]
            share = max(0.0, min(1.0, value / top))
            bar = "█" * round(share * 20) + "░" * (20 - round(share * 20))
            return [cut(f"{label} {bar} {share * 100:.0f}%".strip(), width)]
        if kind in ("Table", "List"):
            rows = self.val(c.get("rows" if kind == "Table" else "items"))
            if not isinstance(rows, list):
                return ["(no rows)" if rows is MISSING else f"(not a list: {cut(shown(rows), 40)})"]
            total = len(rows)
            limit = c.get("limit") or 10
            rows = list(reversed(rows)) if c.get("reverse") else rows
            rows = rows[: min(limit, 5)]
            more = [f"… {total - len(rows)} more rows"] if total > len(rows) else []
            if kind == "List":
                template = c.get("text")
                out = []
                for item in rows:
                    if template and isinstance(item, dict):
                        out.append(cut("• " + TEMPLATE.sub(lambda m: shown(pointer_get(item, m.group(1))) if m.group(1).startswith("/") else m.group(0), template), width))
                    else:
                        out.append(cut("• " + shown(item), width))
                return out + more
            return self.table(c, rows, width) + more
        if kind == "Chart":
            data = self.val(c.get("data"))
            fields = [c["y"]] if isinstance(c.get("y"), str) else c.get("y") or []
            out = [cut(str(c.get("alt", "")).upper(), width)]
            if not isinstance(data, list) or not data:
                return out + ["(no points)"]
            for field in fields:
                series = [as_number(p.get(field)) if isinstance(p, dict) else as_number(p) for p in data]
                values = [v for v in series if v is not None]
                last = f"  last {num(values[-1])}" if values else ""
                out.append(cut(f"{field} {spark(series)}{last}", width))
            out.append(f"({len(data)} points)")
            return out
        if kind == "Image":
            return [cut(f"[image {c.get('file')}: {c.get('alt', '')}]", width)]
        if kind == "Code":
            title = c.get("file") or "code"
            text = self.text_of(c["text"]) if "text" in c else self.extras.get("code", {}).get(cid, "")
            lines = text.splitlines()
            return [cut(f"{title}{':' + c['lines'] if c.get('lines') else ''}", width)] + \
                   ["  " + cut(line, width - 2) for line in lines[:8]] + ([f"  … {len(lines) - 8} more lines"] if len(lines) > 8 else [])
        if kind == "Diff":
            diff = self.extras.get("diffs", {}).get(cid, {})
            patch = c.get("patch") or diff.get("patch", "")
            body = [line for line in patch.splitlines() if line[:1] in "+-" and not line.startswith(("+++", "---"))]
            added = sum(1 for line in body if line.startswith("+"))
            head = f"{c.get('file') or 'patch'} against {c.get('against') or 'HEAD'}  +{added} -{len(body) - added}"
            if diff.get("error"):
                return [cut(head, width), cut(f"  ({diff['error']})", width)]
            return [cut(head, width)] + ["  " + cut(line, width - 2) for line in body[:10]] + \
                   ([f"  … {len(body) - 10} more lines"] if len(body) > 10 else [])
        if kind == "TextField":
            value = self.val(c.get("value"))
            text = "" if value in (MISSING, None) else str(value)
            return [cut(f"{label + ': ' if label else ''}[ {text or c.get('placeholder', '')} ]", width)]
        if kind == "Html":
            file = f" ({c['file']})" if isinstance(c.get("file"), str) else ""
            return [cut(f"[html: {c.get('alt', '')}]{file}", width)]
        return [cut(f"[{kind}: {c.get('alt', 'not supported')}]", width)]

    def table(self, c, rows, width):
        columns = c.get("columns")
        if not columns:
            first = next((r for r in rows if isinstance(r, dict)), {})
            columns = list(first)[:8]
        keys = [col if isinstance(col, str) else col["key"] for col in columns]
        heads = [(col if isinstance(col, str) else col.get("label") or col["key"]).upper() for col in columns]
        cells = [[cut(shown(r.get(k, MISSING)) if isinstance(r, dict) else shown(r), 24) for k in keys] for r in rows]
        widths = [max([len(heads[i])] + [len(row[i]) for row in cells]) for i in range(len(keys))]
        shown_cols, used = 0, 0
        for w in widths:
            if used + w + (2 if shown_cols else 0) > width:
                break
            used += w + (2 if shown_cols else 0)
            shown_cols += 1
        shown_cols = max(1, shown_cols)
        hidden = len(keys) - shown_cols
        line = lambda parts: "  ".join(p.ljust(widths[i]) for i, p in enumerate(parts[:shown_cols])).rstrip()
        out = [line(heads) + (f"  +{hidden} columns" if hidden else "")]
        out += [line(row) for row in cells]
        return [cut(text, width) for text in out]

    def render(self, title, tag):
        head = f"◇ {title}"
        out = [head + " " * max(1, WIDTH - len(head) - len(tag)) + tag]
        out += ["  " + line for line in self.lines("root", WIDTH - 2)]
        return "\n".join(out)


# ---------------------------------------------------------------- files

def now():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def read_json(path):
    with open(path, "r", encoding="utf-8") as f:
        return json.load(f)


def write_json(path, value):
    directory = os.path.dirname(path)
    os.makedirs(directory, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=".v-", suffix=".tmp", dir=directory)
    try:
        os.fchmod(fd, 0o644)
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            json.dump(value, f, ensure_ascii=False, indent=1)
            f.write("\n")
        os.replace(temporary, path)
    except BaseException:
        try:
            os.unlink(temporary)
        except OSError:
            pass
        raise


def previous(ws, session, surface):
    """The newest version of the surface this session has, and its path:
    its own, or else — in a fork — the last one its transcript names."""
    own = f".harness/ui/{session}/{surface}/latest.json"
    if os.path.exists(os.path.join(ws, own)):
        state = read_json(os.path.join(ws, own))
        return state, f".harness/ui/{session}/{surface}/v{state.get('version', 0)}.json"
    log = os.path.join(ws, ".harness", "sessions", f"{session}.session.jsonl")
    try:
        with open(log, "r", encoding="utf-8", errors="replace") as f:
            text = f.read()
    except OSError:
        return None, None
    found = None
    for m in MARKER.finditer(text):
        if m.group(1) == surface:
            found = m.group(4)
    if found and os.path.exists(os.path.join(ws, found)):
        return read_json(os.path.join(ws, found)), found
    return None, None


def git_diff(ws, file, against):
    """The diff of a file against a commit, as the call sees it."""
    if not re.fullmatch(r"[A-Za-z0-9_./~^@{}-]{1,100}", against) or against.startswith("-"):
        return {"error": f"{against}: not a commit"}
    try:
        out = subprocess.run(["git", "-C", ws, "diff", "--no-color", "--no-ext-diff", against, "--", file],
                             capture_output=True, text=True, timeout=20)
        if out.returncode != 0:
            return {"error": out.stderr.strip().splitlines()[0] if out.stderr.strip() else "git diff failed"}
        patch = out.stdout
        if not patch:
            tracked = subprocess.run(["git", "-C", ws, "ls-files", "--error-unmatch", "--", file],
                                     capture_output=True, text=True, timeout=20)
            if tracked.returncode != 0 and os.path.isfile(os.path.join(ws, file)):
                new = subprocess.run(["git", "-C", ws, "diff", "--no-color", "--no-index", "--", "/dev/null", file],
                                     capture_output=True, text=True, timeout=20)
                patch = new.stdout
        if len(patch) > MAX_PATCH:
            return {"patch": patch[:MAX_PATCH], "cut": True}
        return {"patch": patch}
    except (OSError, subprocess.SubprocessError) as e:
        return {"error": str(e)}


def code_text(ws, file, lines):
    try:
        with open(os.path.join(ws, file), "r", encoding="utf-8", errors="replace") as f:
            text = f.read(MAX_TEXT)
    except OSError:
        return ""
    m = re.fullmatch(r"(\d+)(?:-(\d+))?", lines or "")
    if m:
        start = int(m.group(1))
        end = int(m.group(2) or start)
        return "\n".join(text.splitlines()[start - 1:end])
    return text


# ---------------------------------------------------------------- the call

def fail(surface, errors, warnings=()):
    print(f"UI {surface or '?'}: nothing shown; fix these and call UI again:")
    for text in errors:
        print(f"- {text}")
    for text in warnings:
        print(f"warning: {text}")
    sys.exit(2)


def loose(value, what, errors):
    """A model may send a JSON value as a string; take it as the value."""
    if isinstance(value, str):
        try:
            return json.loads(value)
        except ValueError:
            errors.append(f"{what}: not JSON")
    return value


def main():
    raw = sys.stdin.read()
    try:
        args = json.loads(raw) if raw.strip() else None
    except ValueError as e:
        fail("", [f"the arguments are not JSON: {e}"])
    if not isinstance(args, dict):
        fail("", ["the arguments are one JSON object"])
    surface = args.get("surface")
    if not isinstance(surface, str) or not SURFACE_ID.fullmatch(surface):
        fail("", ['surface: an ID of lowercase letters, digits, "-" and "_", as "bench"'])

    ws = os.path.realpath(os.environ.get("KOU_CONVEYOR_WORKSPACE") or os.getcwd())
    session = re.sub(r"[^A-Za-z0-9_-]", "_", os.environ.get("KOU_CONVEYOR_SESSION_ID") or "local")[:80]
    errors, warnings = [], []
    for key in args:
        if key not in ("surface", "title", "glance", "sources", "components", "remove", "data", "delete"):
            warnings.append(f'"{key}" is not an argument of UI, and does nothing')

    # One lock for the session's surfaces: calls made at once take turns.
    # The surface's folder is made only when it gets a version.
    base = os.path.join(ws, ".harness", "ui", session, surface)
    os.makedirs(os.path.dirname(base), exist_ok=True)
    with open(os.path.join(os.path.dirname(base), ".lock"), "a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        before, before_path = previous(ws, session, surface)
        version = int((before or {}).get("version", 0)) + 1
        relative = f".harness/ui/{session}/{surface}/v{version}.json"
        alive = before is not None and not before.get("deleted")

        if args.get("delete") is True:
            if not alive:
                fail(surface, [f'there is no surface "{surface}" to delete'])
            state = {"kou": FORMAT, "surface": surface, "version": version, "session": session,
                     "call": os.environ.get("KOU_CONVEYOR_TOOL_CALL_ID", ""), "at": now(),
                     "title": before.get("title", surface), "deleted": True, "previous": before_path}
            write_json(os.path.join(ws, relative), state)
            write_json(os.path.join(base, "latest.json"), state)
            print(f"UI {surface} v{version} deleted ({relative})")
            return

        state = json.loads(json.dumps(before)) if alive else {
            "title": surface, "glance": "", "sources": {}, "components": [], "data": {}}
        changed = {"added": [], "updated": [], "removed": [], "data": [], "sources": []}

        if "title" in args:
            if isinstance(args["title"], str) and args["title"].strip():
                changed["title"] = state["title"] != args["title"].strip()
                state["title"] = args["title"].strip()
            else:
                errors.append("title: text")
        if "glance" in args:
            if isinstance(args["glance"], str):
                state["glance"] = args["glance"].strip()
            else:
                errors.append("glance: text, as \"p99 {#p99}\"")

        sources = loose(args.get("sources", {}), "sources", errors)
        if not isinstance(sources, dict):
            errors.append('sources: { "<name>": { "file": "path", "format": "jsonl" } }')
            sources = {}
        for name, decl in sources.items():
            where = f"sources.{name}"
            if not SOURCE_NAME.fullmatch(name):
                errors.append(f"{where}: a name of letters, digits, - and _")
                continue
            if decl is None:
                state["sources"].pop(name, None)
                changed["sources"].append(name)
                continue
            if isinstance(decl, str):
                decl = {"file": decl}
            if not isinstance(decl, dict) or "file" not in decl:
                errors.append(f'{where}: {{ "file": "path in the workspace", "format": "{"|".join(FORMATS)}" }}')
                continue
            try:
                file = workspace_path(ws, decl["file"])
            except Invalid as e:
                errors.append(f"{where}.file: {e}")
                continue
            fmt = decl.get("format") or infer_format(file)
            if fmt not in FORMATS:
                errors.append(f"{where}.format: one of {', '.join(FORMATS)}")
                continue
            state["sources"][name] = {"file": file, "format": fmt}
            changed["sources"].append(name)

        components = loose(args.get("components", []), "components", errors)
        if not isinstance(components, list):
            errors.append("components: a list of components")
            components = []
        index = {c["id"]: i for i, c in enumerate(state["components"])}
        placed = {}  # id → where it came in this call
        for i, c in enumerate(components):
            where = f"components[{i}]"
            if not isinstance(c, dict):
                errors.append(f'{where}: {{ "id": "…", "component": "Text", … }}')
                continue
            cid = c.get("id")
            if not isinstance(cid, str) or not COMPONENT_ID.fullmatch(cid):
                errors.append(f"{where}.id: an ID of letters, digits, '.', '-' and '_'")
                continue
            if not isinstance(c.get("component"), str):
                errors.append(f'{where}.component: a component of the catalog, as "Text"')
                continue
            if cid in placed:
                errors.append(f'{where}.id: "{cid}" comes twice in this call (first at {placed[cid]})')
                continue
            placed[cid] = where
            if cid in index:
                state["components"][index[cid]] = c
                changed["updated"].append({"id": cid, "component": c["component"]})
            else:
                index[cid] = len(state["components"])
                state["components"].append(c)
                changed["added"].append({"id": cid, "component": c["component"]})

        remove = loose(args.get("remove", []), "remove", errors)
        if not isinstance(remove, list):
            errors.append("remove: a list of component IDs")
            remove = []
        for j, cid in enumerate(remove):
            if cid in placed:
                errors.append(f'remove[{j}]: "{cid}" is also in components')
            elif cid not in index:
                warnings.append(f'remove[{j}]: there is no component "{cid}"')
            else:
                changed["removed"].append({"id": cid, "component": state["components"][index[cid]].get("component")})
        drop = {c["id"] for c in changed["removed"]}
        state["components"] = [c for c in state["components"] if c["id"] not in drop]

        data = loose(args.get("data", {}), "data", errors)
        if not isinstance(data, dict):
            errors.append('data: { "/path": value }')
            data = {}
        for pointer, value in data.items():
            where = f"data[{json.dumps(pointer, ensure_ascii=False)}]"
            try:
                tokens = tokens_of(pointer)
                if tokens and tokens[0] in ("sources", "sourceInfo"):
                    raise Invalid(f"{pointer} is a file's: change the file, or declare another source")
                state["data"] = pointer_set(state["data"], pointer, value)
                changed["data"].append(pointer)
            except (Invalid, ValueError) as e:
                errors.append(f"{where}: {e}")

        ids = {c["id"] for c in state["components"]}
        metrics = {c["id"] for c in state["components"] if c.get("component") == "Metric"}
        check = Checker(ws, ids, metrics, set(state["sources"]))
        for c in state["components"]:
            check.component(placed.get(c["id"], f'component "{c["id"]}"'), c)
        if state.get("glance"):
            check.template("glance", state["glance"])
        errors += check.errors
        warnings += check.warnings

        comps = {c["id"]: c for c in state["components"]}
        if "root" not in comps:
            errors.append('there is no component "root": a surface starts at it, as '
                          '{ "id": "root", "component": "Column", "children": [ … ] }')
        else:
            reached, path = set(), []

            def walk(cid):
                if cid in path:
                    errors.append(f'component "{cid}" contains itself: ' + " → ".join(path[path.index(cid):] + [cid]))
                    return
                if cid in reached or cid not in comps:
                    return
                reached.add(cid)
                path.append(cid)
                c = comps[cid]
                kids = list(c.get("children") or []) if isinstance(c.get("children"), list) else []
                if isinstance(c.get("tabs"), list):
                    kids += [t.get("child") for t in c["tabs"] if isinstance(t, dict)]
                for kid in kids:
                    if isinstance(kid, str):
                        walk(kid)
                path.pop()

            walk("root")
            loose_ids = [cid for cid in comps if cid not in reached]
            if loose_ids:
                warnings.append("not under root, so not shown: " + ", ".join(f'"{cid}"' for cid in loose_ids))
        if errors:
            fail(surface, errors, warnings)

        # What the files say now: for the snapshot, the warnings and the render.
        values, infos = {}, {}
        for name, decl in state["sources"].items():
            value, info = read_source(ws, decl)
            infos[name] = info
            if value is not MISSING:
                values[name] = value
            if info.get("error") == "no such file":
                warnings.append(f"sources.{name}: there is no {decl['file']} yet; the surface shows it once it appears")
            elif info.get("error"):
                warnings.append(f"sources.{name}: {decl['file']}: {info['error']}")
        doc = {**state["data"], "sources": values, "sourceInfo": infos}

        extras = {"diffs": {}, "code": {}}
        for c in state["components"]:
            where = placed.get(c["id"], f'component "{c["id"]}"')
            kind = c.get("component")
            if kind in ("Image", "Code", "FileLink", "Diff", "Html") and isinstance(c.get("file"), str):
                if not os.path.exists(os.path.join(ws, c["file"])) and kind != "Diff":
                    warnings.append(f"{where}.file: there is no {c['file']}")
            if kind == "Diff" and "patch" not in c and isinstance(c.get("file"), str):
                extras["diffs"][c["id"]] = git_diff(ws, c["file"], c.get("against") or "HEAD")
                if extras["diffs"][c["id"]].get("error"):
                    warnings.append(f"{where}: {extras['diffs'][c['id']]['error']}")
            if kind == "Code" and "file" in c and "text" not in c:
                extras["code"][c["id"]] = code_text(ws, c["file"], c.get("lines"))
            prop = SHOWN.get(kind)
            value = c.get(prop) if prop else None
            if is_binding(value) and pointer_get(doc, value["path"]) is MISSING:
                tokens = tokens_of(value["path"])
                missing_file = tokens[:1] == ["sources"] and len(tokens) > 1 and tokens[1] not in values
                if not missing_file:
                    warnings.append(f"{where}.{prop}: {value['path']} finds nothing now")

        tag = f"{surface} v{version}"
        text = TextRender(state, doc, extras).render(state["title"], tag)
        out = {
            "kou": FORMAT, "surface": surface, "version": version, "session": session,
            "call": os.environ.get("KOU_CONVEYOR_TOOL_CALL_ID", ""), "at": now(),
            "title": state["title"], "glance": state.get("glance", ""),
            "sources": state["sources"], "components": state["components"], "data": state["data"],
            "previous": before_path if alive else None, "changed": changed,
            "snapshot": {"sources": trimmed(values, infos), "sourceInfo": infos, "diffs": extras["diffs"]},
            "text": text,
        }
        write_json(os.path.join(ws, relative), out)
        write_json(os.path.join(base, "latest.json"), out)

    print(f"UI {surface} v{version} ({relative})")
    for line in warnings:
        print(f"warning: {line}")
    print()
    print(text)


if __name__ == "__main__":
    main()
