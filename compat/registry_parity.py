#!/usr/bin/env python3
"""Tool registry, alias, profile and output-bound parity; translators are inert labels and nothing is executed."""
import collections
import json
import os
from pathlib import Path
import random
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
GOLDEN = ROOT / "crates/kou-runtime/src/tool/definitions/static_definitions.json"

STATIC = ["Bash", "Read", "Edit", "Write", "apply_patch", "ViewImage", "TranscriptSearch", "Code", "SkillUse"]
SLOTS = [name for name in STATIC if name != "SkillUse"]
PROFILE_TOOLS = {
    "edit": ["Bash", "Read", "Edit", "Write", "ViewImage", "TranscriptSearch", "SkillUse"],
    "patch": ["Bash", "Read", "apply_patch", "ViewImage", "TranscriptSearch", "SkillUse"],
    "code": ["Code"],
    "shell": ["Bash", "ViewImage", "TranscriptSearch", "SkillUse"],
}
MODELS = [
    "claude-opus-5", "anthropic/claude-sonnet-4.6", "gpt-6-astra", "openai/gpt-5.1-codex", "o3-pro", "codex-mini",
    "gemini-3", "", "llama-4", "ollama/gpt-oss", "GPT-5", " ChatGPT-4o ", "o1", "o", "o0", "o9", "o10", "oa", "O3",
    "openrouter/openai/o4-mini", "a/b/", "/", "//", "gpt", "/gpt", "gpt/", "gpt/claude", "codex", "Codex-Mini",
    "chatgpt", "chat-gpt", "o:", "o/", "x/o1", "o１", "ｇpt", " gpt-5 ", "​gpt", "gPT",
    "gİpt", "CHATGPTİ", "o١", "opus", "ollama/llama", "openai/o", "ＧＰＴ", "\tO1-MINI\n",
    "openai\\gpt-5", "claude/GPT", "GPT/claude", " / gpt-5", "gpt-5 / ", "K/o3", "codex\u0000", "\u0000codex",
]
# Scalars where Go's one-to-one lower-casing, space trimming or %q quoting could part from Rust's.
PROFILE_NAMES = [
    "", " ", "\t\n", "auto", "Auto", " AUTO ", "edit", "EDIT\n", "\tpatch", "code", " Code ", "shell", "Shell ",
    "bogus", "e dit", "edit,patch", "auto,", "edİt", "EDİT", "shİll", "K", "patcℎ",
    "PATCH ", "  shell　", "\u0085code\u0085", " auto ", " edit ",
    "​shell", "﻿auto", "᠎code", "\u001fauto", "auto\u001c", "ΑΣ", "ΌΣΟΣ",
    "STRASSE", "ẞ", "ǅ", "Ⅷ", "Ⓐ", "\U00010400", "\U0001e900", "Ａ", "Ა", "Ꭰ",
    "\"quoted\"", "back\\slash", "new\nline", "tab\there", "\u0000", "\u007f", "x\u0085y", " x", "édit",
    "\U0001f642", "­", "͸", "", "\U000e0001", "�", "a" * 300, "İ" * 3, "CODĖ",
    "Edit‍", "ＥＤＩＴ", "éDIT", "ÉDIT", "İ", "ı", "ſhell", "ſ",
]


def registry(enabled, steps, configured=()):
    slots = [slot if isinstance(slot, dict) else {"Name": slot} for slot in configured]
    return {"Action": "registry", "Enabled": list(enabled), "Configured": slots, "Steps": steps}


def resolve(name, arguments="{}"):
    return {"Op": "resolve", "Name": name, "Arguments": arguments}


def unavailable(name):
    return {"Op": "unavailable", "Name": name}


def register(name, translator="plugin", parameters=None, description="", waits=False, kind="function"):
    tool = {"Type": kind, "Name": name, "Description": description}
    if parameters is not None:
        tool["Parameters"] = parameters
    return {"Op": "register", "Tool": tool, "Translator": translator, "Waits": waits}


def skill(name, path, description="Review code.", manual=False):
    return {"Op": "register_skill", "Skill": {"Name": name, "Description": description, "Path": path, "Manual": manual}}


DEFINITIONS, SKILLS = {"Op": "definitions"}, {"Op": "skills"}
EVERYTHING = registry(STATIC, [DEFINITIONS])
# The registry keeps a registered schema as it was given, so the fixtures give it as
# Go's deterministic encoding writes it: sorted names, whole numbers, no whitespace.
SCHEMA = {"properties": {"path": {"description": "A \"quoted\" path\n界", "type": "string"},
                         "count": {"default": 3, "maximum": 10, "minimum": 0, "type": "integer"}},
          "required": ["path"], "type": "object", "additionalProperties": False}


def selection_cases():
    """Every enabled set of the built-in tools, named in a scrambled order with repeats and strangers."""
    rows, rng = [], random.Random(20261111)
    for mask in range(1 << len(STATIC)):
        chosen = [name for index, name in enumerate(STATIC) if mask >> index & 1]
        enabled = chosen + chosen[::2] + ["GitGlance", "", "read", " Bash"][: mask % 5]
        rng.shuffle(enabled)
        configured = [{"Name": name, "Waits": (mask >> index) & 1 == 1}
                      for index, name in enumerate(SLOTS) if (mask * 37 + index * 11) % 3]
        steps = [DEFINITIONS] + [resolve(name) for name in STATIC] + [unavailable("Grep")]
        steps += [register("GitGlance", "plugin GitGlance", SCHEMA, "Shows the state of the repository.", mask % 2 == 0)]
        steps += [register(name, f"dynamic {name}") for name in STATIC]
        steps += [DEFINITIONS] + [resolve(name) for name in STATIC + ["GitGlance", "gitglance"]]
        steps += [resolve(f"mcp__srv__w_{name}") for name in STATIC] + [unavailable("x_y_GitGlance"), unavailable("Grep")]
        rows.append(registry(enabled, steps, configured))
    return rows


def profile_cases():
    rows = []
    for tools in PROFILE_TOOLS.values():
        for enabled in [tools, [name for name in tools if name != "SkillUse"], tools[::-1]]:
            steps = [DEFINITIONS] + [resolve(name) for name in STATIC] + [unavailable(name) for name in STATIC + ["x_Edit"]]
            rows.append(registry(enabled, steps, SLOTS))
    union = ["Bash", "Read", "Edit", "Write", "ViewImage", "TranscriptSearch", "SkillUse", "apply_patch", "Code"]
    rows += [registry(union, [DEFINITIONS], SLOTS), EVERYTHING]
    rows.append(registry([], [DEFINITIONS, unavailable("Bash"), resolve("Bash")], SLOTS))
    return rows


ALIAS_NAMES = [
    "Read", "grace_Read", "assault_kidney__grace_Read", "mcp__assault_kidney__grace_Read", "apply_patch",
    "grace_apply_patch", "mcp__x__y_apply_patch", "read", "Readx", "_Read", "grace_", "grace__", "patch",
    "grace_patch", "", "_", "__", "___", "_Read_", "Read_", "__Read", "x__Read", "x_Read", "x_Read_Read",
    "Read_Read", "x_apply_patch", "apply_patch_patch", "x_apply__patch", "apply__patch", "x_apply_", "é_Read",
    "界_Read", "_界_Read", "x_Réad", "x_ Read", "x_Read ", "x _Read", "🙂_Bash", "x_Bash\u0000", "\u0000_Bash",
    "x_bash", "X_BASH", "mcp__server__Read", "mcp___Read", "mcp____Read", "a_b_c_Read", "Read_x_Read", "x_notes_Read",
    "notes_Read", "y_x_notes_Read", "x_notes__Read", "x__notes_Read", "_notes_Read", "x_Bash_Read", "x_Read_Bash",
    "q_界_é_z", "界_é_z", "é_z", "z", "x_z", "x_é_z", "x_́_z", "​_Read", "x_\"q\"", "x_a\\b", "x_new\nline",
    "tab\t_Read", "x_Read\n", "x_𐐀", "𐐀_x_𐐀", "a_", "_a", "a_b", "a__b", "a___b", "_a_b", "a_b_", "__a__b__",
    "Bash", "Bash_Bash", "Bash_", "_Bash", "x_Code", "x_SkillUse", "x_TranscriptSearch", "x_ViewImage", "x_Write",
    "x_Edit", "Edit_Write", "Write_Edit_Read", "x" * 300 + "_Read", "_" * 300 + "Read", "Read" + "_Read" * 60,
]


def alias_cases():
    rows = []
    pools = [
        ["Read", "apply_patch", "Bash"], ["Read", "notes_Read"], ["Read", "notes_Read", "x_notes_Read"], [],
        ["z", "é_z", "界_é_z"], ["Read", "Read"], ["_Read", "Read_", "__Read", "Read"], ["patch", "apply_patch"],
        ["", "_", "a", "b", "a_b"], ["\"q\"", "a\\b", "new\nline", "𐐀", "x_𐐀"], STATIC,
    ]
    for names in pools:
        rows += [{"Action": "aliases", "Name": name, "Names": names} for name in ALIAS_NAMES]
    # The registries of the Go tests, then names a gateway could disguise in every way above.
    plain = ["Read", "apply_patch", "Bash"]
    rows.append(registry(plain, [resolve(name) for name in ALIAS_NAMES] + [unavailable(name) for name in ALIAS_NAMES], plain))
    for extra in [["notes_Read"], ["notes_Read", "x_notes_Read"], ["b_Read", "a_b_Read", "_Read", "Read_"],
                  ["z", "é_z", "界_é_z"], ["\"q\"", "a\\b", "new\nline", "𐐀", "x_𐐀"], ["patch", "x_apply_patch"]]:
        steps = [register(name, f"plugin {name}") for name in extra] + [DEFINITIONS]
        steps += [resolve(name) for name in ALIAS_NAMES + ["z_a_b_Read"]]
        steps += [unavailable(name) for name in ALIAS_NAMES + ["z_a_b_Read", "Grep"]]
        rows.append(registry(["Read", "apply_patch"], steps, ["Read"]))
    rows.append(registry([], [unavailable(name) for name in ALIAS_NAMES]))
    # A built-in tool left out is no name at all: its own name may then stand for a plugin's.
    rows.append(registry(["Read"], [register("patch", "plugin patch"), resolve("apply_patch"), resolve("x_apply_patch"),
                                    unavailable("apply_patch"), register("Search", "plugin Search"), resolve("TranscriptSearch"),
                                    resolve("Transcript_Search"), unavailable("y_Transcript_Search")], ["Read", "apply_patch"]))
    rng = random.Random(20261112)
    tokens = ["Read", "apply_patch", "Bash", "notes", "patch", "apply", "mcp", "x", "é", "界", "_", "__", "", " ", "Code"]
    for _ in range(160):
        word = lambda: "".join(rng.choice(tokens) for _ in range(rng.randrange(1, 6)))
        enabled = [name for name in STATIC if rng.random() < 0.4]
        steps = [register(word(), f"plugin {index}", waits=rng.random() < 0.3) for index in range(rng.randrange(0, 6))]
        calls = [word() for _ in range(12)] + [f"{word()}_{rng.choice(STATIC)}" for _ in range(4)]
        steps += [DEFINITIONS] + [resolve(name) for name in calls] + [unavailable(name) for name in calls]
        rows.append(registry(enabled, steps, [name for name in SLOTS if rng.random() < 0.5]))
        rows.append({"Action": "aliases", "Name": word(), "Names": [word() for _ in range(rng.randrange(0, 8))]})
    return rows


def register_cases():
    blank = ["", " ", "\t\n", " ", "\u0085", " ", "  ", "  ", "  　", " 　\t"]
    solid = ["​", "﻿", "᠎", "\u001f", "\u0000", "\u007f", "⁠", "­", " x ", "\"q\"", "a\\b",
             "new\nline", "界", "🙂", "\U000e0001", "́", "é", "x" * 400]
    steps = []
    for name in blank + solid:
        # Order of the checks: the name, the translator, a built-in name, a name already taken.
        steps += [register(name, ""), register(name), register(name), register(name, "")]
    steps += [register("Bash"), register("Bash", ""), register("Read"), register("Read"), register("Read", ""),
              register("bash"), register(" Bash"), register("Bash "), register("SkillUse"), register("SkillUse", ""),
              register("Code", "dynamic Code", SCHEMA, "Runs code.", True), register("Code"),
              register("hosted", "plugin hosted", None, "", False, "hosted"), register("empty", "plugin empty", {}),
              DEFINITIONS]
    steps += [resolve(name) for name in solid + ["Bash", "Read", "Code", "SkillUse", "x_Code", "x_Read", "x_\"q\"", "y_x_界"]]
    steps += [unavailable(name) for name in blank + solid]
    rows = [registry(["Bash", "SkillUse"], steps, ["Bash", "Read"]), registry(STATIC, steps), registry([], steps, SLOTS)]
    # A built-in name left out of the selection is free once, and then taken like any other.
    for name in STATIC:
        enabled = [other for other in STATIC if other != name]
        steps = [resolve(name), unavailable(name), register(name, f"dynamic {name}", SCHEMA, f"Replaces {name}."),
                 register(name, f"second {name}"), DEFINITIONS, resolve(name), resolve(f"x_{name}"), resolve(f"x__{name}"),
                 register(f"y_{name}", f"plugin y_{name}"), resolve(f"x_y_{name}"), unavailable(f"x_y_{name}"), DEFINITIONS]
        rows += [registry(enabled, steps, SLOTS), registry([], steps), registry([name], steps)]
    return rows


def skill_cases():
    review, docs = skill("review", "/skills/review/SKILL.md"), skill("documents", "/skills/docs/SKILL.md", "Edit documents.", True)
    call = lambda name: json.dumps({"name": name}, ensure_ascii=False)
    steps = [SKILLS, resolve("SkillUse", call("review")), review, docs, SKILLS, review,
             skill("review", "/other/SKILL.md"), skill("other", "/skills/review/SKILL.md"), skill("", "/x"),
             skill("x", ""), skill("x", "/x", ""), skill(" 　", "/x"), skill("x", " "), skill("x", "/x", "\t"),
             skill(" review ", "/padded/SKILL.md"), skill("Review", "/skills//review/SKILL.md"), skill("界\n\"q\"", "/界/SKILL.md"),
             SKILLS, resolve("SkillUse", call("review")), resolve("SkillUse", call("documents")),
             resolve("SkillUse", call(" review ")), resolve("SkillUse", call("missing")), resolve("SkillUse", call("界\n\"q\"")),
             resolve("SkillUse", ""), resolve("SkillUse", " "), resolve("SkillUse", "{}"), resolve("x_SkillUse", call("review")),
             resolve("mcp__srv__w_SkillUse", call("review")), unavailable("x_SkillUse"),
             {"Op": "unregister_skill", "Index": 2}, SKILLS, {"Op": "unregister_skill", "Index": 2},
             {"Op": "unregister_skill", "Index": -1}, {"Op": "unregister_skill", "Index": -2}, {"Op": "unregister_skill", "Index": 5},
             SKILLS, resolve("SkillUse", call("review")), review, SKILLS, resolve("SkillUse", call("review")),
             {"Op": "unregister_skill", "Index": 3}, {"Op": "unregister_skill", "Index": 14}, {"Op": "unregister_skill", "Index": 15},
             {"Op": "unregister_skill", "Index": 16}, {"Op": "unregister_skill", "Index": 37}, SKILLS, DEFINITIONS]
    rows = [registry(["SkillUse"], steps), registry(STATIC, steps, SLOTS), registry([], steps)]
    # With SkillUse left out, a plugin may take the name; the catalog stays the registry's.
    rows.append(registry(["Bash"], [review, register("SkillUse", "plugin SkillUse"), resolve("SkillUse", call("review")),
                                    resolve("x_SkillUse", call("review")), SKILLS, DEFINITIONS]))
    return rows


def profile_rows():
    rows = [{"Action": "catalog", "Models": MODELS}, {"Action": "models", "Models": MODELS}]
    rows += [{"Action": "profile", "Name": name, "Models": MODELS[:12]} for name in PROFILE_NAMES]
    rows += [{"Action": "models", "Models": [name]} for name in PROFILE_NAMES]
    for name in ["auto", "edit", "patch", "code", "shell"]:
        rows += [{"Action": "profile", "Name": variant, "Models": MODELS}
                 for variant in [name, name.upper(), name.title(), f"　{name}\t", f" {name.swapcase()} "]]
    return rows


def sweep_rows():
    # Each scalar value alone is a profile name: Go's space, lower-case and printable
    # tables decide the outcome. 4352 blocks of 256, compared by digest.
    return [{"Action": "sweep", "From": start, "To": start + 256} for start in range(0, 0x110000, 256)]


def output_rows():
    rows = []
    texts = ["", "x", "tool \"x\" is not available", "é" * 9, "界" * 40, "a\nb\tc", "🙂" * 7, "x" * 40_000,
             "x" * 40_001, "é" * 40_001, "ab" + "界" * 40_000 + "yz", "\u0000\u007f", "é" * 6]
    for text in texts:
        for limit in [0, -1, -40_000, 1, 2, 3, 4, 7, 10, 20, 39_999, 40_000, 40_001, 1_000_000]:
            if len(text) > 100 and limit not in (0, -1, 3, 39_999, 40_000, 40_001):
                continue
            rows.append({"Action": "error_status", "Message": text, "Limit": limit})
    values = ["", "20000", '"20000"', "20000.0", "1", "1000000", "1000001", '"1000000"', "0", "-1", "-0", "1.5", '"1.5"',
              '"ten"', "true", "false", "null", "[]", "{}", "[1]", '{"n":1}', "999999999999999999999999", "9007199254740992",
              "9007199254740993", "9007199254740994", "-9007199254740994", "1e3", '"1e3"', "1e6", "1e7", '"0x10"', '"0x1p4"',
              '" 12 "', '"1_000"', '""', '" "', '"NaN"', '"Inf"', '"-Infinity"', "1e999", '"1e999"', "0.5", "1e-3",
              '"\\u0031\\u0032"', '"12abc"', '"' + "9" * 60 + '"', '"' + "é" * 60 + '"', "4e4", "40000.0000", '"+7"', '".5e1"']
    # The value is quoted back cut at 40 bytes, which may fall inside a character.
    values += [f'"{"9" * digits}{scalar}x"' for digits in range(34, 41) for scalar in ["é", "界", "🙂", "́"]]
    rows += [{"Action": "max_output_length", "Encoded": value} for value in values]
    return rows


# Go's json/v2 picks "cannot" or "unable to" once for each process; nothing else may differ.
MODAL = re.compile(
    r"decode max_output_length: json: (?:cannot|unable to) unmarshal JSON (?:array|object|string|number|boolean) "
    r"into Go toolarg\.Number: want a number, such as 100, not .*",
    re.DOTALL,
)


def equivalent(go, rust, request):
    if go == rust:
        return True
    if request.get("Action") != "max_output_length":
        return False
    expected, actual = json.loads(go), json.loads(rust)
    left, right = expected.pop("Error"), actual.pop("Error")
    return (expected == actual and MODAL.fullmatch(left) is not None and MODAL.fullmatch(right) is not None
            and left.replace("json: unable to unmarshal", "json: cannot unmarshal", 1)
            == right.replace("json: unable to unmarshal", "json: cannot unmarshal", 1))


OPS = ["definitions", "resolve", "unavailable", "register", "register_skill", "unregister_skill", "skills"]
OUTCOMES = {
    "ambiguous": "may stand for any of", "listed": "; the tools are ", "unconfigured": "is not configured",
    "built in": "is built in", "taken": "is already registered", "no translator": "has no translator",
    "unnamed": "tool name must be set", "skill-use name": "does not match static tool",
}
LABELLED = re.compile(r"translated by (?:static|dynamic) (.+) as (.+)", re.DOTALL)


def tally(seen, asked, step):
    """Counts what a registry step reached, from Go's answer."""
    seen[asked["Op"]] += 1
    status = (step["Status"] or {}).get("Error", "")
    for outcome, marker in OUTCOMES.items():
        seen[outcome] += any(marker in text for text in (step["Error"], step["Text"], status))
    resolved = LABELLED.fullmatch(status)
    seen["alias"] += resolved is not None and resolved[1] != resolved[2]


def self_test():
    request = {"Action": "max_output_length", "Encoded": '"ten"'}
    message = 'decode max_output_length: json: cannot unmarshal JSON string into Go toolarg.Number: want a number, such as 100, not "ten"'
    line = lambda error, limit=0: json.dumps({"Error": error, "Limit": limit})
    other = message.replace("json: cannot", "json: unable to")
    assert equivalent(line(other), line(message), request) and equivalent(line(message), line(other), request)
    assert not equivalent(line(other, 1), line(message), request)
    assert not equivalent(line(other), line(message), {"Action": "profile"})
    for before, after in [("max_output_length", "timeout"), ("toolarg.Number", "string"), ("such as 100", "such as 10"),
                          ('"ten"', '"two"'), ("JSON string", "JSON array")]:
        assert not equivalent(line(other.replace(before, after)), line(message), request)
    assert not equivalent(line("prefix " + other), line("prefix " + message), request)
    assert not equivalent(line("max_output_length must be a positive integer"), line(message), request)


def main():
    self_test()
    groups = {
        "enabled sets": selection_cases(), "profile registries": profile_cases(), "alias and resolution": alias_cases(),
        "registration": register_cases(), "skills": skill_cases(), "profiles": profile_rows(),
        "scalar blocks": sweep_rows(), "output bounds": output_rows(),
    }
    rows = [row for group in groups.values() for row in group]
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "tmp/rust-rewrite-target"))
    subprocess.run(["go", "build", "-o", "tmp/kou-registry-oracle", "./cmd/kou-registry-oracle"], cwd=ROOT, check=True)
    subprocess.run(["cargo", "build", "--locked", "-q", "-p", "kou-runtime", "--example", "registry_oracle"], cwd=ROOT, check=True)
    payload = "".join(json.dumps(row, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n" for row in rows)

    def run(command):
        reply = subprocess.run(command, input=payload.encode(), capture_output=True, cwd=ROOT, check=True)
        lines = reply.stdout.decode().split("\n")
        assert lines.pop() == "" and len(lines) == len(rows), f"{command[0]} answered {len(lines)} of {len(rows)} cases"
        return lines

    go, rust = run([str(ROOT / "tmp/kou-registry-oracle")]), run([str(target / "debug/examples/registry_oracle")])
    failures = worded = 0
    seen = collections.Counter()
    for index, (request, expected, actual) in enumerate(zip(rows, go, rust, strict=True)):
        # Output lines are compared as text: field order and escaping are part of the contract.
        if not equivalent(expected, actual, request):
            failures += 1
            print(f"FAIL {index}: {json.dumps(request, ensure_ascii=False)[:2000]}\nGo:   {expected[:4000]}\nRust: {actual[:4000]}")
            if failures >= 8:
                break
        worded += expected != actual
        answered = json.loads(expected)
        assert answered["Error"] == "" or request["Action"] in ("profile", "max_output_length"), answered["Error"]
        if request["Action"] == "registry":
            for asked, step in zip(request["Steps"], answered["Steps"], strict=True):
                tally(seen, asked, step)
    assert failures == 0, f"{failures} registry projections differ"
    # The corpus must reach every outcome it is meant to compare.
    missing = [outcome for outcome in ["definitions", "alias"] + list(OUTCOMES) if not seen[outcome]]
    assert not missing, f"no case reached {missing}"
    # The Rust unit tests pin the same text without Go; --update-golden rewrites their copy from Go.
    everything = json.loads(go[rows.index(EVERYTHING)])["Steps"][0]["Definitions"]
    if "--update-golden" in sys.argv[1:]:
        GOLDEN.write_text(everything + "\n")
    assert GOLDEN.read_text().rstrip("\n") == everything, f"{GOLDEN} is not what Go encodes"
    counts = ", ".join(f"{len(group)} {name}" for name, group in groups.items())
    reached = ", ".join(f"{seen[outcome]} {outcome}" for outcome in ["alias", "ambiguous", "unconfigured", "built in", "taken", "no translator", "unnamed"])
    print(f"registry parity: {len(rows)} Go/Rust projections ({counts}); {len(rows) - worded} output lines identical as text, "
          f"{worded} differing only in Go's per-process cannot/unable-to wording; {sum(seen[op] for op in OPS)} registry steps "
          f"including {seen['definitions']} serialized definition lists ({reached}); nothing executed")


if __name__ == "__main__":
    main()
