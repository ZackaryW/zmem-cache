"""Rehearse stopped-daemon schema upgrade and compatible backup restore."""
import json
from contextlib import closing
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import time
from tiered_history import make_history

ROOT=Path(__file__).resolve().parents[1]
OLD=ROOT/"target/corrected-predecessor/zmem-svc.exe"
NEW=ROOT/"target/adaptive/debug/zmem-svc.exe"


def main():
    with tempfile.TemporaryDirectory(prefix="zmem-rollout-") as directory:
        root=Path(directory);repo=root/"repo";home=root/"home";home.mkdir()
        head=make_history(repo,4,0,1)
        env=os.environ.copy();env["ZMEM_HOME"]=str(home)
        env["ZMEM_EXTENSION_HOST"]=str(ROOT.parent/"zmem/.venv/Scripts/zmem-extension-host.exe")
        def call(binary,*args):
            return subprocess.run([binary,*map(str,args)],env=env,text=True,capture_output=True,timeout=15)
        def ready(binary):
            for _ in range(100):
                result=call(binary,"query",repo,"--commit-limit",4,"--timeout-ms",10000)
                if result.returncode==0:return json.loads(result.stdout)
                assert json.loads(result.stderr)["code"]=="not_ready",result.stderr
                time.sleep(.05)
            raise AssertionError("fixture did not become ready")
        def backup(source,target):
            with closing(sqlite3.connect(source)) as reader,closing(sqlite3.connect(target)) as writer:reader.backup(writer)
        db=home/"db/entries.db";saved=root/"schema-5.db"
        try:
            before=ready(OLD);assert len(before["entries"])==4
            assert call(OLD,"stop","--timeout-ms",10000).returncode==0
            with closing(sqlite3.connect(db)) as connection:
                key=json.dumps({"path":str(repo),"reference":None,"observed_oid":"f"*40,"commit_limit":None,"node_limit":None,"generation":"rehearsal"})
                connection.execute("INSERT INTO index_jobs VALUES('rehearsal-failure',?,'failed','uncertain fixture')",(key,))
                connection.commit()
            backup(db,saved)
            after=ready(NEW)
            assert after["summary"]["trail"]["resolved_oid"]==head
            assert after["entries"]==before["entries"]
            assert after["summary"]["trail"]["schema_version"]==6
            failure=json.loads(call(NEW,"job-status","rehearsal-failure").stdout)
            assert failure["state"]=="failed"
            assert call(NEW,"stop","--timeout-ms",10000).returncode==0
            rejected=call(OLD,"serve")
            assert rejected.returncode!=0 and "schema 6" in rejected.stderr,rejected.stderr
            backup(saved,db)
            restored=ready(OLD)
            assert restored["summary"]["trail"]["schema_version"]==5
            assert restored["entries"]==before["entries"]
            assert call(OLD,"stop","--timeout-ms",10000).returncode==0
            print(json.dumps({"upgrade_schema":6,"exact_entries_preserved":4,"durable_failure_preserved":True,"older_writer_rejected":True,"restored_schema":5}))
        finally:
            call(NEW,"stop","--timeout-ms",10000)
            call(OLD,"stop","--timeout-ms",10000)


if __name__=="__main__":main()
