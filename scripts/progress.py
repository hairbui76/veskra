#!/usr/bin/env python3
"""Machine-readable ticket tracker for Veskra (progress/tickets.json).

  python3 scripts/progress.py init                 # (re)build from ROADMAP.md, keeping existing status/evidence
  python3 scripts/progress.py set ID STATUS [--evidence TEXT]... [--blocker TEXT]... [--note TEXT]
  python3 scripts/progress.py summary              # per-phase status counts
  python3 scripts/progress.py check                # consistency rules (CI)

Rules enforced by `check` (MASTER_PLAN §12, AGENTS.md §4):
  * status in the allowed set;
  * Done requires >= 1 evidence entry and every dependency Done;
  * Blocked requires >= 1 blocker entry;
  * In Verification is used when implementation exists but required evidence (HW/ADOBE/...) is missing.
"""
import json, re, sys, argparse, pathlib, datetime

ROOT = pathlib.Path(__file__).resolve().parent.parent
TRACKER = ROOT / "progress" / "tickets.json"
STATUSES = ["Backlog", "Ready", "In Progress", "Blocked", "PR Review", "In Verification", "Done", "Deferred"]
PHASE_RE = re.compile(r"^## Phase (P\d)")
ROW_RE = re.compile(r"^\| \[[ x]\] \*\*([A-Z]+-\d+)\*\* (.+?) \| (.+?) \| (.+?) \| (.+?) \|\s*$")


def expand_deps(text):
    text = text.strip()
    if text in ("—", "-", ""):
        return []
    out = []
    for part in [p.strip() for p in text.split(",")]:
        m = re.match(r"^([A-Z]+)-(\d+)…(?:([A-Z]+)-)?(\d+)$", part)
        if m:
            pre, a, _, b = m.groups()
            width = len(a)
            out += [f"{pre}-{i:0{width}d}" for i in range(int(a), int(b) + 1)]
        elif part:
            out.append(part)
    return out


def parse_roadmap():
    phase = None
    tickets = []
    for line in (ROOT / "ROADMAP.md").read_text(encoding="utf-8").splitlines():
        m = PHASE_RE.match(line)
        if m:
            phase = m.group(1)
            continue
        m = ROW_RE.match(line)
        if m and phase:
            tid, title, owner, deps, deliverable = m.groups()
            tickets.append({"id": tid, "title": title.strip(), "phase": phase, "owner": owner.strip(),
                            "depends": expand_deps(deps), "deliverable": deliverable.strip()})
    return tickets


def load():
    return json.loads(TRACKER.read_text(encoding="utf-8")) if TRACKER.exists() else {"tickets": []}


def save(data):
    data["updated"] = datetime.date.today().isoformat()
    TRACKER.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def cmd_init(_):
    old = {t["id"]: t for t in load().get("tickets", [])}
    out = []
    for t in parse_roadmap():
        prev = old.get(t["id"], {})
        t["status"] = prev.get("status", "Backlog")
        t["evidence"] = prev.get("evidence", [])
        t["blockers"] = prev.get("blockers", [])
        t["notes"] = prev.get("notes", [])
        out.append(t)
    save({"schema": 1, "source": "ROADMAP.md", "statuses": STATUSES, "tickets": out})
    print(f"{len(out)} tickets")


def cmd_set(a):
    data = load()
    by = {t["id"]: t for t in data["tickets"]}
    if a.id not in by:
        sys.exit(f"unknown ticket {a.id}")
    if a.status not in STATUSES:
        sys.exit(f"bad status {a.status}; one of {STATUSES}")
    t = by[a.id]
    t["status"] = a.status
    t["evidence"] += a.evidence or []
    if a.clear_blockers:
        t["blockers"] = []
    t["blockers"] += a.blocker or []
    t["notes"] += a.note or []
    save(data)


def cmd_summary(_):
    data = load()
    phases = {}
    for t in data["tickets"]:
        phases.setdefault(t["phase"], {}).setdefault(t["status"], []).append(t["id"])
    for p in sorted(phases):
        counts = ", ".join(f"{s}: {len(v)}" for s, v in sorted(phases[p].items(), key=lambda kv: STATUSES.index(kv[0])))
        print(f"{p}: {counts}")


def cmd_check(_):
    data = load()
    by = {t["id"]: t for t in data["tickets"]}
    errs = []
    for t in data["tickets"]:
        if t["status"] not in STATUSES:
            errs.append(f"{t['id']}: invalid status {t['status']}")
        for d in t["depends"]:
            if d not in by:
                errs.append(f"{t['id']}: unknown dependency {d}")
        if t["status"] == "Done":
            if not t["evidence"]:
                errs.append(f"{t['id']}: Done without evidence")
            for d in t["depends"]:
                if d in by and by[d]["status"] != "Done":
                    errs.append(f"{t['id']}: Done but dependency {d} is {by[d]['status']}")
        if t["status"] == "Blocked" and not t["blockers"]:
            errs.append(f"{t['id']}: Blocked without blocker description")
    if errs:
        print("\n".join(errs))
        sys.exit(1)
    print(f"tracker OK ({len(data['tickets'])} tickets)")


ap = argparse.ArgumentParser()
sub = ap.add_subparsers(required=True)
sub.add_parser("init").set_defaults(f=cmd_init)
s = sub.add_parser("set")
s.add_argument("id"); s.add_argument("status")
s.add_argument("--evidence", action="append"); s.add_argument("--blocker", action="append")
s.add_argument("--note", action="append"); s.add_argument("--clear-blockers", action="store_true")
s.set_defaults(f=cmd_set)
sub.add_parser("summary").set_defaults(f=cmd_summary)
sub.add_parser("check").set_defaults(f=cmd_check)
args = ap.parse_args()
args.f(args)
