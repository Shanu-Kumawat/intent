#!/usr/bin/env python3
"""Runtime spec-conformance verification for the `intent` binary.
Every check cites the SPEC.md section it verifies."""
import json, os, shutil, subprocess, sys, tempfile, concurrent.futures

import os as _os
BIN = _os.environ.get("INTENT_BIN", "./target/debug/intent")
DB_DIR = tempfile.mkdtemp(prefix="intent-spec-")
DB = os.path.join(DB_DIR, "tasks.db")

results = []
def check(name, spec, ok, detail=""):
    results.append((name, spec, ok, detail))
    print(f"{'PASS' if ok else 'FAIL'}  [{spec}] {name}" + (f"  — {detail}" if detail and not ok else ""))

def run(*args, db=None, extra_env=None):
    env = dict(os.environ, INTENT_DB=db or DB)
    if extra_env:
        env.update(extra_env)
    p = subprocess.run([BIN, *args], capture_output=True, text=True, env=env, timeout=30)
    return p.returncode, p.stdout, p.stderr

def jrun(*args, **kw):
    if "--json" not in args:
        args = (*args, "--json")
    code, out, err = run(*args, **kw)
    if out.strip():
        try:
            return code, json.loads(out), err
        except Exception:
            raise RuntimeError(f"bad JSON from {args}: rc={code} out={out!r} err={err!r}")
    raise RuntimeError(f"empty stdout from {args}: rc={code} err={err!r}")

def err_json(stderr):
    try:
        return json.loads(stderr)["error"]
    except Exception:
        return None

# fresh db
for f in (DB, DB+"-wal", DB+"-shm"):
    if os.path.exists(f): os.remove(f)

# ---------- §1.1 / §3: JSON contract ----------
code, t, err = jrun("add", "Alpha task", "--json")
FIELDS = ["id","title","notes","status","inbox","priority","project","tags",
          "due","remind","defer_until","created_at","updated_at","completed_at"]
check("task JSON: exactly the 14 spec fields in spec order", "§3", code==0 and list(t.keys())==FIELDS, str(list(t.keys())))
import string
ALPHA = set("23456789abcdefghjkmnpqrstuvwxyz")
check("id: 6 chars from the spec alphabet", "§1", len(t["id"])==6 and set(t["id"])<=ALPHA, t["id"])
check("capture defaults: active + inbox", "§1.1/§1.2", t["status"]=="active" and t["inbox"] is True)
check("capture timestamps are RFC3339 UTC (Z)", "§1", t["created_at"].endswith("Z"))
alpha_id = t["id"]

# ---------- §1.2: inbox semantics ----------
code, t, _ = jrun("edit", alpha_id, "--priority", "high", "--json")
check("priority edit does NOT clear inbox", "§1.2", t["inbox"] is True)
code, t, _ = jrun("edit", alpha_id, "--tags", "x,y", "--json")
check("tags edit does NOT clear inbox", "§1.2", t["inbox"] is True)
code, t, _ = jrun("edit", alpha_id, "--notes", "body", "--json")
check("notes edit does NOT clear inbox", "§1.2", t["inbox"] is True)
code, t, _ = jrun("edit", alpha_id, "--remind", "tomorrow", "--json")
check("remind edit DOES clear inbox", "§1.2", t["inbox"] is False)
code, t2, _ = jrun("add", "Beta due", "--json")
jrun("edit", t2["id"], "--due", "tomorrow", "--json")
code, t3, _ = jrun("add", "Gamma proj", "--json")
jrun("edit", t3["id"], "--project", "lab", "--json")
inbox = jrun("list", "inbox", "--json")[1]
check("inbox empty after due/remind/project edits", "§1.2", len(inbox)==0)

# ---------- §1.3: view classification ----------
jrun("add", "plain inbox", "--json")                      # inbox
code, t, _ = jrun("add", "future due", "--json"); jrun("edit", t["id"], "--due", "+10d")
code, t, _ = jrun("add", "due today", "--json");  jrun("edit", t["id"], "--due", "today")
code, t, _ = jrun("add", "remind today", "--json"); jrun("edit", t["id"], "--remind", "today")
code, t, _ = jrun("add", "deferred", "--json"); jrun("edit", t["id"], "--due", "yesterday"); jrun("defer", t["id"], "+5d")
code, t, _ = jrun("add", "done one", "--json"); jrun("done", t["id"])
code, t, _ = jrun("add", "archived one", "--json"); jrun("archive", t["id"])

def titles(view):
    return {x["title"] for x in jrun("list", view, "--json")[1]}
ti = titles("inbox")
check("inbox = active AND inbox flag", "§1.3", ti=={"plain inbox"}, str(ti))
tt = titles("today")
check("today = due≤today OR remind≤today, defer hides", "§1.3",
      tt=={"due today","remind today"}, str(tt))
tu = titles("upcoming")
check("upcoming = active, not inbox, not today (future-dated, reminded, organized, undated)", "§1.3",
      tu=={"future due","deferred","Alpha task","Beta due","Gamma proj"}, str(tu))
tc = titles("completed")
check("completed = status done", "§1.3", tc=={"done one"}, str(tc))
ta = titles("archived")
check("archived = status archived", "§1.3", ta=={"archived one"}, str(ta))
tall = titles("all")
check("all = everything except archived", "§1.3",
      len(tall)==9 and "archived one" not in tall, str(tall))
check("`list` with no arg defaults to all", "§3",
      {x["title"] for x in jrun("list","--json")[1]}==tall)

# sort: today due-ascending (due today < remind today has due? remind-only sorts by remind)
today_list = jrun("list", "today", "--json")[1]
check("today sorted dated-first ascending", "§1.3",
      [x["title"] for x in today_list]==["due today","remind today"],
      str([x["title"] for x in today_list]))

# ---------- §1.4: date grammar ----------
date_ok = {"today":"today","tod":"today","tomorrow":None,"tom":None,"tmr":None,
           "yesterday":None,"+3":None,"+3d":None,"+2w":None,"+1m":None,"+1y":None,
           "-2d":None,"mon":None,"sunday":None,"2026-12-25":"2026-12-25"}
allok = True; detail=[]
for d in date_ok:
    c,_,e = run("add", f"date {d}", f"--due={d}")
    if c != 0: allok=False; detail.append(f"{d}: exit {c} {e.strip()}")
check("all §1.4 date forms accepted via --due=<date> (equals form)", "§1.4", allok, "; ".join(detail))
# weekday resolves forward (mon → next Monday, today included)
c, t, _ = jrun("add", "weekday probe", "--due", "mon", "--json")
from datetime import date, timedelta
today = date.today()
days_ahead = (0 - today.weekday()) % 7
expected = today + timedelta(days=days_ahead)
check("weekday resolves to next occurrence (today included)", "§1.4",
      t["due"]==expected.isoformat(), f"{t['due']} vs {expected}")
bad = 0; badlist=[]
for d in ["gibberish","20261102","+3x","next friday","13/01"]:
    c,_,e = run("add", "x", "--due", d, "--json")
    ej = err_json(e)
    if not (c==4 and ej and ej["code"]=="bad_date"): bad+=1; badlist.append(d)
    run("x") if False else None
check("non-spec date forms rejected: exit 4, code bad_date", "§1.4/§3", bad==0, str(badlist))

# ---------- §3: errors, exit codes, semantics ----------
c,_,e = run("show", "zzzzzz", "--json")
check("missing task: exit 3, code not_found", "§3", c==3 and err_json(e)["code"]=="not_found")
c,_,e = run("add", "x", "--priority", "urgent", "--json")
check("bad priority: exit 4, code bad_value", "§3", c==4 and err_json(e)["code"]=="bad_value")
c,_,e = run("list", "bogus", "--json")
check("unknown view: exit 4", "§3", c==4)
c,_,e = run("add", "x", "--json"); aid = json.loads(_ if False else open("/dev/null").read() or "null") if False else None
c,out,_ = run("add", "tmp task", "--json"); aid = json.loads(out)["id"]
c,_,e = run("edit", aid, "--json")
check("edit with no flags: exit 2 (usage)", "§3", c==2)
c,_,e = run("delete", aid)
ej = err_json(e)
check("delete without --force, non-TTY: refused exit 1", "§3",
      c==1 and "--force" in e, e.strip())
c,out,_ = run("delete", aid, "--force", "--json")
check("delete --force succeeds; JSON acknowledges", "§3", c==0 and json.loads(out).get("deleted") is True)
c,_,e = run("defer", "zzzzzz", "+1d", "--json")
check("defer on missing id: exit 3", "§3", c==3)
# mutating commands echo the resulting task
c,out,_ = run("add", "echo check", "--json"); eid = json.loads(out)["id"]
c,out,_ = run("done", eid, "--json")
check("mutation echoes resulting task", "§3", c==0 and json.loads(out)["status"]=="done")
# search excludes archived
c,out,_ = run("add", "findme please", "--json"); sid=json.loads(out)["id"]
run("archive", sid)
c,hits,_ = jrun("search", "findme", "--json")
check("search excludes archived", "§3", len(hits)==0)

# ---------- §3: counts + machine surfaces ----------
c, cnt, _ = jrun("counts", "--json")
check("counts --json: per-view map", "§3",
      isinstance(cnt, dict) and set(cnt)>= {"inbox","today","upcoming","completed","archived","all"})

# ---------- §2: storage ----------
xdg = os.path.join(DB_DIR, "xdg")
env_no_intent_db = {k: v for k, v in os.environ.items() if k != "INTENT_DB"}
env_no_intent_db["XDG_DATA_HOME"] = xdg
p = subprocess.run([BIN, "add", "xdg probe", "--json"], capture_output=True, text=True,
                   env=env_no_intent_db, timeout=30)
c = p.returncode
xdgdb = os.path.join(xdg, "intent", "tasks.db")
check("default path is $XDG_DATA_HOME/intent/tasks.db", "§2", os.path.exists(xdgdb), xdgdb)
# F2 regression: empty INTENT_DB must fall back to XDG, not a disposable temp db
p = subprocess.run([BIN, "add", "empty intent db probe", "--json"], capture_output=True, text=True,
                   env={**env_no_intent_db, "INTENT_DB": ""}, timeout=30)
check("empty INTENT_DB falls back to the default path (no temp-db data loss)", "§2",
      p.returncode == 0 and os.path.exists(xdgdb) and json.loads(p.stdout) is not None)
import sqlite3
conn = sqlite3.connect(DB)
mode = conn.execute("PRAGMA journal_mode").fetchone()[0]
check("database is in WAL mode", "§2", mode.lower()=="wal", mode)
ver = conn.execute("SELECT value FROM meta WHERE key='schema_version'").fetchone()[0]
check("schema version recorded", "§2", ver=="1", ver)
conn.close()

# ---------- §2: concurrency — parallel writers + readers ----------
N=16
for f in (DB, DB+"-wal", DB+"-shm"): pass  # keep existing db
def add_one(i):
    return run("add", f"concurrent {i}", "--json")[0]
with concurrent.futures.ThreadPoolExecutor(max_workers=N) as ex:
    codes = list(ex.map(add_one, range(N)))
check(f"{N} parallel `intent add` processes all succeed (WAL)", "§2", all(c==0 for c in codes), str(codes))
c, all_tasks, _ = jrun("list", "all", "--json")
conc = [x for x in all_tasks if x["title"].startswith("concurrent ")]
check("all concurrent writes durable and readable", "§2", len(conc)==N, f"{len(conc)}/{N}")
conn = sqlite3.connect(DB)
integrity = conn.execute("PRAGMA integrity_check").fetchone()[0]
conn.close()
check("sqlite integrity_check passes after stress", "§2", integrity=="ok", integrity)

# ---------- summary ----------
fails = [r for r in results if not r[2]]
print(f"\n{len(results)-len(fails)}/{len(results)} checks passed")
if fails:
    print("\nFAILURES:")
    for name, spec, _, detail in fails:
        print(f"  [{spec}] {name}: {detail}")
shutil.rmtree(DB_DIR, ignore_errors=True)
sys.exit(1 if fails else 0)
