"""Matched workload for a schema-5 eager or schema-6 adaptive service.

Promotion state is a controlled persisted fixture, not a shortened live clock.
All measured operations retain their actual end-to-end process deadlines.
"""
from __future__ import annotations
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import statistics
import subprocess
import tempfile
import time
import venv


def run_workload(args, source, make_history, percentile):
    results = []
    for repetition in range(args.repetitions):
        with tempfile.TemporaryDirectory(prefix="zmem-adaptive-bench-") as directory:
            root = Path(directory)
            binary = root / "runtime" / "binary" / source.name
            binary.parent.mkdir(parents=True)
            shutil.copy2(source, binary)
            host = root / "runtime" / "host"
            venv.EnvBuilder(with_pip=False).create(host)
            python = host / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
            purelib = Path(subprocess.check_output([python,"-c","import sysconfig;print(sysconfig.get_path('purelib'))"],text=True).strip())
            project = Path(__file__).resolve().parents[1]
            shutil.copytree(project.parent/"zmem"/"src"/"zmem",purelib/"zmem",ignore=shutil.ignore_patterns("__pycache__","*.pyc","_native"))
            home = root / "home"
            home.mkdir()
            (home/"config.toml").write_text("background_commit_limit=10000\nstaging_max_bytes=450000\nprotect_recent_days=0\n")
            env = os.environ.copy()
            env["ZMEM_HOME"] = str(home)
            for name in ("ZMEM_EXTENSION_HOST","PYTHONPATH","PYTHONHOME","PYTHONUSERBASE"):
                env.pop(name,None)
            db = home/"db"/"entries.db"

            def call(*argv, message=None):
                started = time.perf_counter()
                process = subprocess.run([binary,*map(str,argv)],input=message,env=env,capture_output=True,text=True,timeout=125)
                elapsed = (time.perf_counter()-started)*1000
                value = json.loads(process.stdout if process.returncode==0 else process.stderr)
                return elapsed,process.returncode,value

            def ready(repo, depth, reference=None):
                started = time.perf_counter()
                refargs = ("--ref",reference) if reference else ()
                _,code,value = call("query",repo,"--commit-limit",depth,"--include-invalid","--timeout-ms",10000,*refargs)
                while code:
                    assert value.get("code")=="not_ready",value
                    assert time.perf_counter()-started < 120,value
                    job = value["job_id"]
                    while True:
                        _,status_code,status=call("job-status",job,"--timeout-ms",10000)
                        assert status_code==0,status
                        assert status["state"] not in ("failed","obsolete"),status
                        if status["state"]=="ready": break
                        time.sleep(0.05)
                    _,code,value=call("query",repo,"--commit-limit",depth,"--include-invalid","--timeout-ms",10000,*refargs)
                assert value["summary"]["trail"]["selected_commits"]==depth,value
                return (time.perf_counter()-started)*1000,value

            try:
                identity = call("version-json")[2]
                adaptive = identity["schema_version"]>=6
                repos = [root/name for name in ("once","shallow","progressive")]
                heads = [make_history(repo,1600,50,80) for repo in repos]
                def git(repo,*argv,input=None):
                    git_env=env.copy()
                    git_env.update(GIT_AUTHOR_NAME="Bench",GIT_COMMITTER_NAME="Bench",GIT_AUTHOR_EMAIL="bench@example.com",GIT_COMMITTER_EMAIL="bench@example.com",GIT_AUTHOR_DATE="1700000000 +0000",GIT_COMMITTER_DATE="1700000000 +0000")
                    return subprocess.check_output(["git","-C",str(repo),*argv],input=input,text=True,env=git_env).strip()
                effect_target=git(repos[2],"log","--format=%H","--grep=zmem(DECISION)").splitlines()[15]
                tree=git(repos[2],"rev-parse","HEAD^{tree}")
                parent=git(repos[2],"rev-parse","HEAD^")
                heads[2]=git(repos[2],"commit-tree",tree,"-p",parent,input=f"benchmark boundary\n\nzmem(CANCEL)[{effect_target[:12]}, 1]\n")
                git(repos[2],"update-ref","refs/heads/main",heads[2])
                cold=[]
                summaries=[]
                audited_facts={}
                for repo,depth in zip(repos,(500,500,1000)):
                    elapsed,value=ready(repo,depth)
                    cold.append(round(elapsed,1));summaries.append(value["summary"])
                    if args.utilization_audit and not adaptive:
                        deadline=time.monotonic()+30
                        while time.monotonic()<deadline:
                            with closing(sqlite3.connect(db)) as connection:
                                checkpoint=connection.execute("SELECT p.completed_count,p.state FROM prefetch_jobs p JOIN repositories r ON r.id=p.repository_id WHERE r.path=?",(value["summary"]["repository"],)).fetchone()
                            if checkpoint==(1600,"ready"):break
                            time.sleep(.05)
                        assert checkpoint==(1600,"ready"),checkpoint
                        with closing(sqlite3.connect(db)) as connection:
                            for repo_id,oid,byte_count in connection.execute("SELECT f.repository_id,f.commit_oid,f.bytes+COALESCE((SELECT SUM(e.bytes) FROM raw_parent_edges e WHERE e.repository_id=f.repository_id AND e.commit_oid=f.commit_oid),0) FROM raw_commit_facts f JOIN repositories r ON r.id=f.repository_id WHERE r.path=? AND NOT EXISTS(SELECT 1 FROM trail_membership m WHERE m.repository_id=f.repository_id AND m.commit_oid=f.commit_oid)",(value["summary"]["repository"],)):
                                audited_facts[(repo_id,oid)]=byte_count
                assert all(entry["sha"]!=effect_target for entry in value["entries"]),"bounded view unexpectedly includes the cancelled target"
                for _ in range(3):
                    ready(repos[1],500)
                if adaptive:
                    with closing(sqlite3.connect(db)) as connection:
                        assert connection.execute("SELECT COUNT(*) FROM prefetch_jobs").fetchone()[0]==0,"shallow/first request started speculation"
                # Same stop/start cost outside measurement for both binaries.
                assert call("stop","--timeout-ms",10000)[1]==0
                if adaptive:
                    summary=summaries[2]
                    now=int(time.time())
                    demand={"repository":summary["repository"],"route":"refs/heads/main","oid":heads[2],
                            "generation":summary["trail"]["extension_identity"],
                            "observations":[{"at":now-120,"depth":1000},{"at":now-60,"depth":1000}]}
                    payload=json.dumps(demand)
                    with closing(sqlite3.connect(db)) as connection:
                        connection.execute("INSERT OR REPLACE INTO route_demand VALUES(?,?,?,?)",(summary["repository"],demand["route"],payload,len(payload)))
                        connection.commit()
                assert call("ensure","--timeout-ms",10000)[1]==0
                # Warm installed identity before timing the mixed workload.
                ready(repos[1],500)
                ready(repos[2],1000)
                if adaptive:
                    deadline=time.monotonic()+30
                    while time.monotonic()<deadline:
                        with closing(sqlite3.connect(db)) as connection:
                            checkpoint=connection.execute("SELECT completed_count,state FROM prefetch_jobs WHERE head_oid=?",(heads[2],)).fetchone()
                        if checkpoint==(1500,"ready"):break
                        time.sleep(.05)
                    assert checkpoint==(1500,"ready"),checkpoint
                check=subprocess.Popen([binary,"check",repos[1],"--deep","--commit-limit","500","--timeout-ms","120000"],
                    env=env,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
                check.stdin.write("zmem(DECISION): benchmark preview\n")
                check.stdin.close();check.stdin=None
                with ThreadPoolExecutor(max_workers=args.lookup_clients+1) as pool:
                    wider_future=pool.submit(ready,repos[2],1500)
                    samples=list(pool.map(lambda _:call("query",repos[1],"--commit-limit",500,"--timeout-ms",10000),range(args.warm_samples)))
                    wider_ms,wider=wider_future.result()
                for _,code,value in samples:
                    assert code==0,value
                    assert value["summary"]["trail"]["resolved_oid"]==heads[1]
                stdout,stderr=check.communicate(timeout=125)
                assert check.returncode==0,(stdout,stderr)
                target_entry=next(entry for entry in wider["entries"] if entry["sha"]==effect_target)
                assert target_entry["valid"] is False,"wider cross-window CANCEL did not apply"
                with closing(sqlite3.connect(db)) as connection:
                    unused=connection.execute("SELECT COALESCE(SUM(bytes),0) FROM raw_commit_facts r WHERE NOT EXISTS(SELECT 1 FROM trail_membership m WHERE m.repository_id=r.repository_id AND m.commit_oid=r.commit_oid)").fetchone()[0]
                    unused+=connection.execute("SELECT COALESCE(SUM(bytes),0) FROM raw_parent_edges r WHERE NOT EXISTS(SELECT 1 FROM trail_membership m WHERE m.repository_id=r.repository_id AND m.commit_oid=r.commit_oid)").fetchone()[0]
                    metrics=None
                    if adaptive:
                        connection.row_factory=sqlite3.Row
                        metrics=dict(connection.execute("SELECT * FROM prefetch_metrics").fetchone())
                        assert metrics["reused_facts"]==500,metrics
                        assert metrics["produced_facts"]==500,metrics
                        assert unused==0,unused
                times=[row[0] for row in samples]
                result={"repetition":repetition+1,"clients":args.lookup_clients,"samples":len(times),"warm_ms":{
                    "p50":percentile(times,.5),"p95":percentile(times,.95),"p99":percentile(times,.99),"stdev":round(statistics.pstdev(times),1)},
                    "cold_demand_ms":cold,"wider_demand_ms":round(wider_ms,1),"unused_bytes":unused,"metrics":metrics}
                if args.utilization_audit and not adaptive:
                    with closing(sqlite3.connect(db)) as connection:
                        demanded=set(connection.execute("SELECT DISTINCT repository_id,commit_oid FROM trail_membership"))
                    reused=audited_facts.keys() & demanded
                    result["utilization_audit"]={"produced_facts":len(audited_facts),"produced_bytes":sum(audited_facts.values()),"reused_facts":len(reused),"reused_bytes":sum(audited_facts[key] for key in reused)}
                    assert len(audited_facts)==2800 and len(reused)==500,result
                results.append(result)
                print(json.dumps(result),flush=True)
                if repetition == 0:
                    meta=git(repos[2],"commit-tree",tree,"-p",heads[2],input=f"benchmark metadata\n\nzmem(META)[{effect_target[:12]}, {heads[2][:12]}, owner=bench]\n")
                    git(repos[2],"update-ref","refs/heads/meta",meta)
                    _,code,failure=call("query",repos[2],"--ref","meta","--commit-limit",500,"--timeout-ms",10000)
                    assert code and failure["code"]=="not_ready",failure
                    deadline=time.monotonic()+120
                    while time.monotonic()<deadline:
                        _,_,status=call("job-status",failure["job_id"],"--timeout-ms",10000)
                        if status["state"]=="failed":break
                        assert status["state"]!="ready","incomplete META range unexpectedly published"
                        time.sleep(.05)
                    assert status["state"]=="failed",status
                    _,complete=ready(repos[2],1601,"meta")
                    target_entry=next(entry for entry in complete["entries"] if entry["sha"]==effect_target)
                    assert target_entry["owner"]=="bench","wider cross-window META did not apply"
            finally:
                call("stop","--timeout-ms",10000)
    report={"binary":str(source),"sha256":hashlib.sha256(source.read_bytes()).hexdigest(),"identity":identity,
            "fixture":"3 x 1600 merged commits, decision every 80, 450000-byte staging quota; controlled persisted promotion; installed host; concurrent deep check and wider demand",
            "utilization_audit":args.utilization_audit,
            "runs":results}
    if args.output:
        args.output.parent.mkdir(parents=True,exist_ok=True)
        args.output.write_text(json.dumps(report,indent=2)+"\n")
    else:
        print(json.dumps(report,indent=2))
