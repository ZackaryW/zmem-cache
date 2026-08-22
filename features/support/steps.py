from __future__ import annotations

import difflib
import json
import os
import sqlite3
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from behave import given, then, when

from features.support.lifecycle import (
    ZMEM_ROOT,
    assemble_test_runtime,
    commit,
    database,
    init_repo,
    run_svc,
    write_config,
    write_global_extension,
)


def _query(context, include_invalid: bool = True) -> None:
    args = ["query", str(context.repo)]
    if include_invalid:
        args.append("--include-invalid")
    run_svc(context, *args)


def _assert_success(context) -> None:
    assert context.completed.returncode == 0, (context.completed.stdout, context.completed.stderr)


def _entry(context, sha: str) -> dict:
    with database(context) as connection:
        row = connection.execute(
            "SELECT commit_oid,annotation_index,entry_type,score,valid FROM entries WHERE commit_oid=?1",
            [sha],
        ).fetchone()
    assert row is not None
    return {"sha": row[0], "index": row[1], "type": row[2], "score": row[3], "valid": bool(row[4])}


def _persistent_state(context) -> str:
    with database(context) as connection:
        return "\n".join(connection.iterdump())


def _trail_rows(context) -> set[tuple[object, ...]]:
    with database(context) as connection:
        return set(
            connection.execute(
                "SELECT id,repository_id,head_oid,attention_identity,extension_identity,protocol_version,"
                "schema_version,legacy,selected_commit_count,selected_node_count,source_time FROM trails"
            ).fetchall()
        )


def _assert_persistent_state(context) -> None:
    after = _persistent_state(context)
    assert after == context.persistent_before, "\n".join(
        difflib.unified_diff(context.persistent_before.splitlines(), after.splitlines())
    )


@given("an indexed repository with one valid decision")
def given_check_valid_decision(context):
    init_repo(context)
    context.decision_sha = commit(context, "zmem(DECISION): preview target")
    run_svc(context, "add", str(context.repo))
    _assert_success(context)
    context.persistent_before = _persistent_state(context)
    context.trails_before = _trail_rows(context)


@when("I fast-check a proposed cancellation of that decision")
def when_fast_cancel_check(context):
    message = f"feat: cancel preview\n\nzmem(CANCEL)[{context.decision_sha[:8]}, 1]"
    run_svc(context, "check", str(context.repo), input_text=message)
    _assert_success(context)


@then("the service reports the decision would become invalid with score 0.0")
def then_service_projects_cancel(context):
    effect = context.payload["effects"][0]
    assert context.payload["ok"] is True
    assert effect["kind"] == "cancel" and effect["status"] == "applied"
    assert effect["before_valid"] is True and effect["before_score"] == 1.0
    assert effect["after_valid"] is False and effect["after_score"] == 0.0


@then("the stored entry and selected trail remain unchanged after the check")
def then_store_unchanged_after_check(context):
    assert _entry(context, context.decision_sha)["valid"] is True
    with database(context) as connection:
        virtual = connection.execute("SELECT COUNT(*) FROM commits WHERE oid=?1", ["0" * 40]).fetchone()[0]
        invalid = connection.execute(
            "SELECT COUNT(*) FROM trail_entry_state WHERE commit_oid=?1 AND valid=0", [context.decision_sha]
        ).fetchone()[0]
    assert context.trails_before <= _trail_rows(context)
    assert virtual == 0 and invalid == 0


@when("I fast-check a decay followed by cancellation of that decision")
def when_fast_ordered_effects(context):
    prefix = context.decision_sha[:8]
    message = f"feat: ordered preview\n\nzmem(DECAY)[{prefix}, 1, 0.5]\nzmem(CANCEL)[{prefix}, 1]"
    run_svc(context, "check", str(context.repo), input_text=message)
    _assert_success(context)


@then("the projected effects run in annotation order")
def then_effects_are_ordered(context):
    effects = context.payload["effects"]
    assert [effect["kind"] for effect in effects] == ["decay", "cancel"]
    assert effects[0]["after_score"] == 0.5
    assert effects[1]["before_score"] == 0.5 and effects[1]["after_score"] == 0.0


@then("the stored decision remains valid with score 1.0")
def then_stored_decision_unchanged(context):
    entry = _entry(context, context.decision_sha)
    assert entry["valid"] is True and entry["score"] == 1.0


@given("an indexed repository with one cancelled decision")
def given_cancelled_decision(context):
    init_repo(context)
    context.decision_sha = commit(context, "zmem(DECISION): cancelled target")
    commit(context, f"zmem(CANCEL)[{context.decision_sha[:8]}, 1]")
    run_svc(context, "add", str(context.repo))
    _assert_success(context)
    assert _entry(context, context.decision_sha)["valid"] is False


@when("I fast-check a proposed decay of that decision")
def when_fast_decay_cancelled(context):
    message = f"feat: decay cancelled\n\nzmem(DECAY)[{context.decision_sha[:8]}, 1, 0.5]"
    run_svc(context, "check", str(context.repo), input_text=message)
    _assert_success(context)


@then("the effect outcome is a no-op that does not restore the decision")
def then_decay_is_noop(context):
    effect = context.payload["effects"][0]
    assert effect["status"] == "no_op"
    assert effect["before_valid"] is False and effect["after_valid"] is False
    assert _entry(context, context.decision_sha)["valid"] is False


@when("I fast-check a cancellation of a missing target")
def when_fast_missing_cancel(context):
    run_svc(
        context,
        "check",
        str(context.repo),
        input_text="feat: invalid cancellation\n\nzmem(CANCEL)[deadbeef, 1]",
    )
    _assert_success(context)


@then("the check is unsuccessful with a rejected effect diagnostic")
def then_cache_rejected_effect(context):
    assert context.payload["ok"] is False
    assert context.payload["effects"][0]["status"] == "rejected"
    assert "unresolved or ambiguous effect target" in context.payload["diagnostics"]


@given("reachable history whose decision row is absent from the persistent cache")
def given_evicted_decision(context):
    init_repo(context)
    context.decision_sha = commit(
        context,
        "zmem(DECISION): evicted target",
        timestamp="2000-01-01T00:00:00+00:00",
    )
    commit(context, "zmem(LESSON_LEARNT): retained row")
    run_svc(context, "add", str(context.repo))
    _assert_success(context)
    with database(context) as connection:
        connection.execute("DELETE FROM entries WHERE commit_oid=?1", [context.decision_sha])
        assert (
            connection.execute("SELECT COUNT(*) FROM entries WHERE commit_oid=?1", [context.decision_sha]).fetchone()[0]
            == 0
        )


@when("I deep-check a proposed cancellation of that decision")
def when_deep_cancel_evicted(context):
    message = f"feat: deep cancellation\n\nzmem(CANCEL)[{context.decision_sha[:8]}, 1]"
    run_svc(context, "check", str(context.repo), "--deep", input_text=message)
    _assert_success(context)


@then("isolated replay resolves and projects the cancellation")
def then_deep_resolves_cancel(context):
    effect = context.payload["effects"][0]
    assert context.payload["mode"] == "deep" and effect["status"] == "applied"
    assert effect["resolved_sha"] == context.decision_sha and effect["after_valid"] is False


@then("the absent decision is not copied into persistent rows")
def then_deep_does_not_copy(context):
    with database(context) as connection:
        assert (
            connection.execute("SELECT COUNT(*) FROM entries WHERE commit_oid=?1", [context.decision_sha]).fetchone()[0]
            == 0
        )


@given("a trusted repository with an active custom expander and hook")
def given_trusted_preview_extensions(context):
    init_repo(context)
    commit(context, "")
    expander = context.repo / ".zmem" / "extend" / "expanders" / "custom.py"
    hook = context.repo / ".zmem" / "extend" / "hooks" / "observe.py"
    context.hook_marker = context.temp_root / "hook-ran"
    expander.parent.mkdir(parents=True)
    hook.parent.mkdir(parents=True)
    expander.write_text(
        "API_VERSION=1\nclass Custom:\n extension_id='CUSTOM'\n"
        " def expand(self, context): context.add_entry(type='CUSTOM', content=context.annotation.content)\n"
        "def register(registry, mode='extend'): registry.extend('CUSTOM', Custom())\n"
    )
    hook.write_text(
        "from pathlib import Path\nAPI_VERSION=1\n"
        f"def observe(context): Path({str(context.hook_marker)!r}).write_text('ran')\n"
        "def register(registry): registry.register('after_expand', observe)\n"
    )
    run_svc(context, "add", str(context.repo), "--trust-extensions")
    _assert_success(context)


@when("I fast-check its custom annotation")
def when_fast_custom_check(context):
    run_svc(
        context,
        "check",
        str(context.repo),
        input_text="feat: custom preview\n\nzmem(CUSTOM): projected",
    )
    _assert_success(context)


@then("the expander action and extension identity are reported")
def then_custom_action_reported(context):
    assert context.payload["actions"][0]["kind"] == "add_entry"
    assert context.payload["actions"][0]["type"] == "CUSTOM"
    assert context.payload["extension_hash"]


@then("the hook does not run and is reported skipped")
def then_cache_hook_skipped(context):
    assert context.payload["hooks"] == "skipped"
    assert not context.hook_marker.exists()


@given("no zmem service is running for the isolated user home")
def given_stopped_service(context):
    assert not (context.home / "service.json").exists()


@when("an authorized local client ensures the service")
def when_ensure_service(context):
    run_svc(context, "ensure")


@then("it can connect to one per-user service")
def then_one_service(context):
    _assert_success(context)
    state = json.loads((context.home / "service.json").read_text())
    assert context.payload["pid"] == state["pid"] and state["token"]


@given("a Git repository with a supported annotation")
def given_supported_repo(context):
    init_repo(context)
    context.head = commit(context, "zmem(DECISION): choose SQLite")


@when("I run zmem-svc add for its path with trusted extensions")
def when_add_trusted(context):
    run_svc(context, "add", str(context.repo), "--trust-extensions")
    _assert_success(context)
    context.first_add = context.payload
    run_svc(context, "add", str(context.repo), "--trust-extensions")


@then("the canonical repository is registered once")
def then_registered_once(context):
    _assert_success(context)
    with database(context) as connection:
        assert connection.execute("SELECT COUNT(*) FROM repositories").fetchone()[0] == 1


@then("its current HEAD is indexed with extension trust")
def then_indexed_trusted(context):
    with database(context) as connection:
        row = connection.execute(
            "SELECT t.head_oid, r.trusted_extensions FROM trails t JOIN repositories r ON r.id=t.repository_id"
        ).fetchone()
    assert row == (context.head, 1) and context.first_add["indexed_commits"] == 1


@given("a path outside any Git repository")
def given_non_repo(context):
    context.repo.mkdir()


@when("I run zmem-svc add for that path")
def when_add_non_repo(context):
    run_svc(context, "add", str(context.repo))


@then("registration fails without an anchor or entries")
def then_registration_fails_cleanly(context):
    assert context.completed.returncode != 0
    db_path = context.home / "db" / "entries.db"
    if db_path.exists():
        with sqlite3.connect(db_path) as connection:
            assert connection.execute("SELECT COUNT(*) FROM anchors").fetchone()[0] == 0
            assert connection.execute("SELECT COUNT(*) FROM entries").fetchone()[0] == 0


@given("a registered repository whose HEAD advances")
def given_advanced_repo(context):
    init_repo(context)
    commit(context, "zmem(DECISION): first")
    run_svc(context, "add", str(context.repo))
    _assert_success(context)
    context.head = commit(context, "zmem(LESSON_LEARNT): second")


@when("a client queries the new HEAD")
def when_query_advanced(context):
    _query(context)


@then("the service indexes through that HEAD before responding")
def then_query_is_fresh(context):
    _assert_success(context)
    assert context.payload["summary"]["head"] == context.head
    assert context.payload["summary"]["indexed_commits"] == 1
    assert {entry["content"] for entry in context.payload["entries"]} == {"first", "second"}


@when("an authorized local client ensures and inspects the service")
def when_ensure_and_status(context):
    run_svc(context, "ensure")
    _assert_success(context)
    context.ensure_payload = context.payload
    run_svc(context, "status")


@then("status reports one running release with its protocol identity")
def then_status_identity(context):
    _assert_success(context)
    assert context.payload["running"] is True
    assert context.payload["pid"] == context.ensure_payload["pid"]
    assert context.payload["release_version"]
    assert context.payload["protocol_version"] == 4


@given("an alternate zmem home and a separate unused default home")
def given_alternate_home(context):
    assert Path(context.env["ZMEM_HOME"]) == context.home
    assert context.default_user != context.home


@then("service state exists only beneath the alternate home")
def then_only_alternate_state(context):
    _assert_success(context)
    assert (context.home / "service.json").exists()
    assert not (context.default_user / ".zmem" / "service.json").exists()


@given("a service binary assembled with a sibling Python host")
def given_assembled_runtime(context):
    assemble_test_runtime(context)


@when("I query through the assembled service without a host override")
def when_query_assembled(context):
    _query(context)


@then("the supported annotation is indexed by the sibling host")
def then_sibling_host_indexed(context):
    _assert_success(context)
    assert context.payload["entries"][0]["content"] == "choose SQLite"


@when("two authorized clients ensure the service concurrently")
def when_concurrent_ensure(context):
    def ensure():
        return subprocess.run(
            [context.svc, "ensure"],
            env=context.env,
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )

    with ThreadPoolExecutor(max_workers=2) as pool:
        context.concurrent = list(pool.map(lambda _index: ensure(), range(2)))


@then("both clients observe the same healthy service identity")
def then_concurrent_identity(context):
    assert all(result.returncode == 0 for result in context.concurrent), context.concurrent
    payloads = [json.loads(result.stdout) for result in context.concurrent]
    assert len({payload["pid"] for payload in payloads}) == 1, payloads
    run_svc(context, "status")
    _assert_success(context)
    assert context.payload["pid"] == payloads[0]["pid"], (context.payload, payloads)


@given("a commit with a supported entry, unsupported annotation, and valid effect")
def given_mixed_annotations(context):
    init_repo(context)
    first = commit(context, "zmem(DECISION): target")
    run_svc(context, "add", str(context.repo))
    _assert_success(context)
    context.mixed_head = commit(
        context,
        f"zmem(LESSON_LEARNT): learned\nzmem(UNKNOWN): ignored\nzmem(DECAY)[{first[:8]}, 1, 0.5]",
    )


@when("the commit is indexed")
def when_commit_indexed(context):
    _query(context)


@then("one entry is stored and the effect is applied")
def then_mixed_applied(context):
    _assert_success(context)
    current = [row for row in context.payload["entries"] if row["sha"] == context.mixed_head]
    target = next(row for row in context.payload["entries"] if row["content"] == "target")
    assert len(current) == 1 and current[0]["content"] == "learned" and target["score"] == 0.5


@then("the unsupported annotation is diagnosed")
def then_unsupported_diagnosed(context):
    with database(context) as connection:
        messages = [row[0] for row in connection.execute("SELECT message FROM diagnostics")]
    assert any("unsupported annotation type: UNKNOWN" in message for message in messages)


@given("a repository anchored at an ancestor of HEAD")
def given_ancestor_anchor(context):
    init_repo(context)
    commit(context, "zmem(DECISION): base")
    run_svc(context, "add", str(context.repo))
    _assert_success(context)


@when("two descendant commits are synchronized")
def when_two_descendants(context):
    commit(context, "zmem(DECISION): two")
    context.head = commit(context, "zmem(DECISION): three")
    _query(context)


@then("only those two commits are expanded before the anchor advances")
def then_two_expanded(context):
    _assert_success(context)
    assert context.payload["summary"]["indexed_commits"] == 2
    assert context.payload["summary"]["trail"]["resolved_oid"] == context.head


@given("an indexed history that cancels a decision")
def given_cancelled_history(context):
    init_repo(context)
    context.decision_head = commit(context, "zmem(DECISION): keep")
    context.cancel_head = commit(context, f"zmem(CANCEL)[{context.decision_head[:8]}, 1]")
    _query(context)
    _assert_success(context)
    assert context.payload["entries"][0]["valid"] is False
    context.cancelled_trail_id = context.payload["summary"]["trail"]["trail_id"]


@when("HEAD is rewritten without the cancellation")
def when_rewrite_without_cancel(context):
    subprocess.run(
        ["git", "-C", context.repo, "reset", "--hard", context.decision_head], check=True, capture_output=True
    )
    commit(context, "no annotation", subject="docs: rewritten")
    _query(context)


@then("repository state is rebuilt and the decision is valid")
def then_rebuilt_valid(context):
    _assert_success(context)
    decision = next(row for row in context.payload["entries"] if row["content"] == "keep")
    assert decision["valid"] is True and decision["score"] == 1.0
    assert context.payload["summary"]["indexed_commits"] == 1
    assert context.payload["summary"]["trail"]["trail_id"] != context.cancelled_trail_id


@given("indexing fails fatally after proposing an effect")
def given_failing_range(context):
    init_repo(context)
    context.base = commit(context, "zmem(DECISION): stable")
    run_svc(context, "add", str(context.repo))
    _assert_success(context)
    context.base_trail_count = 1
    commit(context, f"zmem(DECAY)[{context.base[:8]}, 1, 0.5]")
    write_global_extension(
        context,
        "expanders",
        "bad.py",
        "API_VERSION=1\nclass Bad:\n extension_id='BAD'\n def expand(self, context): raise RuntimeError('fatal expansion')\ndef register(registry, mode='extend'): registry.extend('BAD', Bad())\n",
    )
    commit(context, "zmem(BAD): fail")


@when("the range transaction ends")
def when_range_fails(context):
    _query(context)


@then("neither the effect nor the new anchor is visible")
def then_range_atomic(context):
    assert context.completed.returncode != 0
    with database(context) as connection:
        trail_count = connection.execute("SELECT COUNT(*) FROM trails").fetchone()[0]
        score = connection.execute("SELECT score FROM entries WHERE content='stable'").fetchone()[0]
    assert trail_count == context.base_trail_count and score == 1.0


@given("more ready commit work than max_concurrency")
def given_concurrent_work(context):
    write_config(context, max_concurrency=2, max_entries=3_000_000, protect_recent_days=14)
    init_repo(context)
    for index in range(5):
        commit(context, f"zmem(DECISION): {index}")


@when("the repository is indexed")
def when_repo_indexed(context):
    _query(context)


@then("simultaneous expansion never exceeds the configured bound")
def then_bound_used(context):
    _assert_success(context)
    assert context.payload["summary"]["max_concurrency"] == 2


@then("application remains deterministic")
def then_order_deterministic(context):
    assert [row["content"] for row in context.payload["entries"]] == [str(index) for index in range(5)]


@given("an isolated user home without a zmem database")
def given_no_database(context):
    init_repo(context)
    commit(context, "")
    assert not (context.home / "db" / "entries.db").exists()


@when("the store opens")
def when_store_opens(context):
    run_svc(context, "add", str(context.repo))


@then("a usable entries database exists under .zmem/db")
def then_database_exists(context):
    _assert_success(context)
    with database(context) as connection:
        assert connection.execute("PRAGMA user_version").fetchone() is not None


@given("stored commit cohorts exceed max_entries")
def given_old_cohorts(context):
    write_config(context, max_concurrency=2, max_entries=10, protect_recent_days=0)
    init_repo(context)
    context.old_head = commit(context, "zmem(DECISION): oldest", timestamp="2020-01-01T12:00:00+0000")
    commit(context, "zmem(DECISION): newest", timestamp="2021-01-01T12:00:00+0000")
    _query(context)
    _assert_success(context)


@given("the oldest cohort is eligible by committer time")
def given_oldest_eligible(context):
    db_path = context.home / "db" / "entries.db"
    os.utime(db_path, (2_000_000_000, 2_000_000_000))
    write_config(context, max_concurrency=2, max_entries=1, protect_recent_days=0)


@when("retention runs")
def when_retention_runs(context):
    _query(context)


@then("every row owned by the oldest cohort is removed")
def then_oldest_removed(context):
    _assert_success(context)
    assert [row["content"] for row in context.payload["entries"]] == ["newest"]


@then("database modification time does not affect its ordering")
def then_mtime_ignored(context):
    assert context.payload["entries"][0]["sha"] != context.old_head


@given("protected commits alone exceed max_entries")
def given_protected_over_capacity(context):
    write_config(context, max_concurrency=2, max_entries=1, protect_recent_days=14)
    init_repo(context)
    commit(context, "zmem(DECISION): recent one")
    commit(context, "zmem(DECISION): recent two")


@when("retention runs with protect_recent_days set to 14")
def when_protected_retention(context):
    _query(context)


@then("no protected commit is evicted")
def then_protected_kept(context):
    _assert_success(context)
    assert len(context.payload["entries"]) == 2


@then("the store reports that it remains over capacity")
def then_soft_cap_reported(context):
    assert context.payload["summary"]["over_capacity"] is True


@given("old and recent commit cohorts exceed max_entries")
def given_zero_protection_cohorts(context):
    write_config(context, max_concurrency=2, max_entries=1, protect_recent_days=0)
    init_repo(context)
    commit(context, "zmem(DECISION): old", timestamp="2020-01-01T12:00:00+0000")
    commit(context, "zmem(DECISION): recent")


@when("retention runs with protect_recent_days set to 0")
def when_zero_protection(context):
    _query(context)


@then("cohorts are evicted by committer time until capacity is met")
def then_capacity_met(context):
    _assert_success(context)
    assert len(context.payload["entries"]) == 1 and context.payload["entries"][0]["content"] == "recent"


@given("entries behind a current repository anchor are evicted")
def given_evicted_behind_anchor(context):
    write_config(context, max_concurrency=2, max_entries=1, protect_recent_days=0)
    init_repo(context)
    commit(context, "zmem(DECISION): old", timestamp="2020-01-01T12:00:00+0000")
    context.head = commit(context, "zmem(DECISION): current", timestamp="2021-01-01T12:00:00+0000")
    _query(context)
    _assert_success(context)


@when("that unchanged repository synchronizes again")
def when_unchanged_sync(context):
    _query(context)


@then("its anchored range is not replayed")
def then_not_replayed(context):
    _assert_success(context)
    assert context.payload["summary"]["indexed_commits"] == 0
    assert context.payload["summary"]["head"] == context.head


@given("a compatible extension host journals a valid add-entry context action")
def given_compatible_action(context):
    write_global_extension(
        context,
        "expanders",
        "custom.py",
        "API_VERSION=1\nclass Custom:\n extension_id='CUSTOM'\n def expand(self, context): context.add_entry(type='CUSTOM', content=context.annotation.content)\ndef register(registry, mode='extend'): registry.extend('CUSTOM', Custom())\n",
    )
    init_repo(context)
    commit(context, "zmem(CUSTOM): derived")


@when("the service validates its action journal")
def when_service_validates(context):
    if hasattr(context, "invalid_journal"):
        run_svc(context, "validate-journal", input_text=context.invalid_journal)
    else:
        _query(context)


@then("the service can persist the entry without granting database access")
def then_custom_persisted(context):
    _assert_success(context)
    assert context.payload["entries"][0]["type"] == "CUSTOM"
    assert context.payload["entries"][0]["content"] == "derived"


@given("an extension host response containing data without valid journal provenance")
def given_unjournaled_response(context):
    context.invalid_journal = json.dumps(
        {"protocol_version": 4, "extension_hash": "x", "entries": [{"content": "bypass"}]}
    )


@then("the response is rejected and no anchor advances")
def then_unjournaled_rejected(context):
    assert context.completed.returncode != 0
    assert not (context.home / "db" / "entries.db").exists()


@given("an extension host with an unsupported protocol version")
def given_bad_protocol_host(context):
    init_repo(context)
    context.anchor = commit(context, "zmem(DECISION): stable")
    run_svc(context, "add", str(context.repo))
    _assert_success(context)
    context.protocol_initial_trails = 1
    run_svc(context, "stop")
    script = context.temp_root / "bad_host.py"
    script.write_text(
        "import json,sys\nsys.stdin.buffer.read()\nprint(json.dumps({'protocol_version':99,'extension_hash':'bad','journal':{'version':1,'origin':'zmem-expansion-context','actions':[]}}))\n"
    )
    write_config(
        context,
        max_concurrency=2,
        max_entries=3_000_000,
        protect_recent_days=14,
        extension_host=sys.executable,
        extension_host_args=[str(script)],
    )
    context.env.pop("ZMEM_EXTENSION_HOST", None)
    commit(context, "zmem(DECISION): should not land")


@when("a repository range is indexed")
def when_bad_range_indexed(context):
    _query(context)


@then("indexing fails and its anchor does not advance")
def then_protocol_anchor_unchanged(context):
    assert context.completed.returncode != 0
    with database(context) as connection:
        trails = connection.execute("SELECT COUNT(*) FROM trails").fetchone()[0]
        heads = {row[0] for row in connection.execute("SELECT head_oid FROM trails")}
    assert trails == context.protocol_initial_trails and heads == {context.anchor}


@given("an anchor containing the previous extension-set identity")
def given_previous_identity(context):
    context.extension = write_global_extension(
        context,
        "expanders",
        "custom.py",
        "API_VERSION=1\nVALUE='one'\nclass Custom:\n extension_id='CUSTOM'\n def expand(self, context): context.add_entry(type='CUSTOM', content=VALUE)\ndef register(registry, mode='extend'): registry.extend('CUSTOM', Custom())\n",
    )
    init_repo(context)
    commit(context, "zmem(CUSTOM): stable")
    _query(context)
    _assert_success(context)
    assert context.payload["entries"][0]["content"] == "one"


@when("the current extension host reports a different identity")
def when_identity_changes(context):
    context.extension.write_text(
        "API_VERSION=1\nVALUE='second'\nclass Custom:\n extension_id='CUSTOM'\n def expand(self, context): context.add_entry(type='CUSTOM', content=VALUE)\ndef register(registry, mode='extend'): registry.extend('CUSTOM', Custom())\n"
    )
    _query(context)


@then("repository synchronization selects a rebuild")
def then_rebuild_selected(context):
    _assert_success(context)
    assert context.payload["summary"]["indexed_commits"] == 1
    assert len(context.payload["entries"]) == 1 and context.payload["entries"][0]["content"] == "second"


@given("valid expansion output and a failing hook diagnostic")
def given_failing_hook(context):
    write_global_extension(
        context,
        "hooks",
        "failing.py",
        "API_VERSION=1\ndef fail(context): raise RuntimeError('hook boom')\ndef register(registry): registry.register('after_index', fail)\n",
    )
    init_repo(context)
    commit(context, "zmem(DECISION): canonical")


@when("the service validates the response")
def when_validate_hook_response(context):
    _query(context)


@then("the entry remains valid for commit")
def then_hook_entry_valid(context):
    _assert_success(context)
    assert context.payload["entries"][0]["valid"] is True


@then("the hook diagnostic remains visible")
def then_hook_diagnostic_visible(context):
    with database(context) as connection:
        diagnostics = [row[0] for row in connection.execute("SELECT message FROM diagnostics")]
    assert any("hook boom" in message for message in diagnostics)


def _configure_observable_host(context, mode: str, *, default_concurrency: bool = False) -> None:
    run_svc(context, "stop")
    context.host_state = context.temp_root / f"host-{mode}.state"
    script = context.temp_root / f"host-{mode}.py"
    script.write_text(
        f"""import json, os, sqlite3, sys, time
from pathlib import Path
MODE = {mode!r}
STATE = Path({str(context.host_state)!r})
request = json.loads(sys.stdin.buffer.read())
operation = request['operation']

def count_attempt():
    value = int(STATE.read_text()) if STATE.exists() else 0
    STATE.write_text(str(value + 1))
    return value + 1

def response(**values):
    print(json.dumps({{'protocol_version': 4, **values}}))

if MODE == 'timeout':
    STATE.write_text(str(os.getpid()))
    time.sleep(10)
elif operation == 'identity':
    response(extension_hash='fake-host', journal={{'version': 1, 'origin': 'zmem-expansion-context', 'actions': []}}, hook_diagnostics=[], annotation_count=0)
elif operation in ('inspect', 'inspect_batch'):
    attempt = count_attempt() if MODE != 'concurrency' else 1
    if MODE == 'parser_retry' and attempt == 1:
        raise SystemExit(7)
    items = request.get('items', [{{'id': 'single', 'message': request.get('message', '')}}])
    inspections = [{{'id': item['id'], 'annotation_count': item['message'].count('zmem('), 'parser_diagnostics': []}} for item in items]
    if MODE == 'incomplete' and inspections:
        inspections.pop()
    if operation == 'inspect':
        item = inspections[0]
        response(annotation_count=item['annotation_count'], parser_diagnostics=[])
    else:
        response(inspections=inspections)
elif operation == 'expand':
    if MODE == 'expansion_fail':
        count_attempt()
        raise SystemExit(9)
    if MODE == 'bypass':
        response(extension_hash='fake-host', journal={{'version': 1, 'origin': 'zmem-expansion-context', 'actions': []}}, hook_diagnostics=[], annotation_count=1, entries=[{{'content': 'bypass'}}])
        raise SystemExit(0)
    if MODE == 'concurrency':
        connection = sqlite3.connect(STATE, timeout=10)
        connection.execute('CREATE TABLE IF NOT EXISTS state(active INTEGER NOT NULL, maximum INTEGER NOT NULL)')
        connection.execute('INSERT INTO state(active,maximum) SELECT 0,0 WHERE NOT EXISTS(SELECT 1 FROM state)')
        connection.commit()
        connection.execute('BEGIN IMMEDIATE')
        active, maximum = connection.execute('SELECT active,maximum FROM state').fetchone()
        active += 1
        connection.execute('UPDATE state SET active=?,maximum=?', (active, max(maximum, active)))
        connection.commit()
        time.sleep(0.3)
        connection.execute('BEGIN IMMEDIATE')
        connection.execute('UPDATE state SET active=active-1')
        connection.commit()
        connection.close()
    response(extension_hash='fake-host', journal={{'version': 1, 'origin': 'zmem-expansion-context', 'actions': []}}, hook_diagnostics=[], annotation_count=request['message'].count('zmem('))
else:
    raise SystemExit(11)
"""
    )
    values = {
        "max_entries": 3_000_000,
        "protect_recent_days": 14,
        "extension_host_timeout_seconds": 1 if mode == "timeout" else 30,
        "extension_host": sys.executable,
        "extension_host_args": [str(script)],
    }
    if not default_concurrency:
        values["max_concurrency"] = 2
    write_config(context, **values)
    context.env.pop("ZMEM_EXTENSION_HOST", None)


def _process_exists(pid: int) -> bool:
    if os.name == "nt":
        completed = subprocess.run(
            ["tasklist", "/FI", f"PID eq {pid}", "/NH"], capture_output=True, text=True, check=False
        )
        return str(pid) in completed.stdout
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    return True


@given("an indexed repository and an extension host that outlives its deadline")
def given_timed_host(context):
    init_repo(context)
    context.anchor_before_timeout = commit(context, "zmem(DECISION): stable")
    _query(context)
    _assert_success(context)
    context.timeout_initial_trails = 1
    commit(context, "zmem(DECISION): must not land")
    _configure_observable_host(context, "timeout")


@when("the next repository range is indexed")
def when_timed_range_indexed(context):
    _query(context)


@then("a host-timeout error is returned")
def then_timeout_returned(context):
    assert context.completed.returncode != 0
    assert "timed out" in (context.completed.stdout + context.completed.stderr)


@then("the timed-out host exits without advancing the anchor")
def then_timeout_reaped_and_atomic(context):
    pid = int(context.host_state.read_text())
    assert not _process_exists(pid)
    with database(context) as connection:
        assert connection.execute("SELECT COUNT(*) FROM trails").fetchone()[0] == context.timeout_initial_trails
        assert connection.execute("SELECT head_oid FROM trails").fetchone()[0] == context.anchor_before_timeout


@given("a parser-only host that fails its first attempt and succeeds its second")
def given_retrying_parser_host(context):
    init_repo(context)
    commit(context, "zmem(DECISION): retry parser")
    _configure_observable_host(context, "parser_retry")


@when("repository attention is selected through the service")
def when_attention_selected(context):
    _query(context)


@then("selection succeeds after exactly two parser attempts")
def then_parser_retried_once(context):
    _assert_success(context)
    assert context.host_state.read_text() == "2"


@given("a hook-bearing expansion host that records attempts and fails")
def given_failing_expansion_host(context):
    init_repo(context)
    commit(context, "zmem(DECISION): no retry")
    _configure_observable_host(context, "expansion_fail")


@when("its repository range is indexed")
def when_failing_expansion_indexed(context):
    _query(context)


@then("indexing fails after exactly one expansion attempt")
def then_expansion_not_retried(context):
    assert context.completed.returncode != 0
    assert context.host_state.read_text() == "2"


@then("the failed range does not advance its anchor")
def then_failed_expansion_has_no_anchor(context):
    with database(context) as connection:
        assert connection.execute("SELECT COUNT(*) FROM trails").fetchone()[0] == 0


@given("selected history whose inspection host omits one batch result")
def given_incomplete_batch_host(context):
    init_repo(context)
    commit(context, "zmem(DECISION): first")
    commit(context, "zmem(DECISION): second")
    _configure_observable_host(context, "incomplete")


@then("the incomplete batch is rejected before history selection")
def then_incomplete_batch_rejected(context):
    assert context.completed.returncode != 0
    with database(context) as connection:
        assert connection.execute("SELECT COUNT(*) FROM inspections").fetchone()[0] == 0
        assert connection.execute("SELECT COUNT(*) FROM trails").fetchone()[0] == 0


@given("selected history with multiple uncached commit messages")
def given_ordered_uncached_history(context):
    init_repo(context)
    context.first_inspected = commit(context, "zmem(DECISION): one")
    context.second_inspected = commit(context, "zmem(DECISION): two\nzmem(LESSON_LEARNT): extra")
    _configure_observable_host(context, "ordered")


@then("the inspection batch associates every result with its commit in order")
def then_batch_association_ordered(context):
    _assert_success(context)
    with database(context) as connection:
        counts = dict(connection.execute("SELECT commit_oid,annotation_count FROM inspections"))
    assert counts[context.first_inspected] == 1
    assert counts[context.second_inspected] == 2


@given("more ready host work than the default concurrency")
def given_default_concurrency_work(context):
    init_repo(context)
    for index in range(12):
        commit(context, f"zmem(DECISION): default {index}")
    _configure_observable_host(context, "concurrency", default_concurrency=True)


@when("the repository is indexed without a concurrency override")
def when_default_concurrency_indexed(context):
    _query(context)


@then("simultaneous host execution never exceeds eight")
def then_default_concurrency_bounded(context):
    _assert_success(context)
    with sqlite3.connect(context.host_state) as connection:
        maximum = connection.execute("SELECT maximum FROM state").fetchone()[0]
    assert maximum == 8


@then("the service reports max_concurrency eight")
def then_default_concurrency_reported(context):
    assert context.payload["summary"]["max_concurrency"] == 8


@given("stable history observed through a counting inspection host")
def given_counted_stable_history(context):
    init_repo(context)
    commit(context, "zmem(DECISION): cached")
    _configure_observable_host(context, "counting")


@when("the repository is queried twice without parser or history changes")
def when_stable_history_queried_twice(context):
    _query(context)
    _assert_success(context)
    context.first_attention = context.payload["summary"]["attention"]
    context.first_inspection_attempts = int(context.host_state.read_text())
    _query(context)


@then("the second selection starts no inspection hosts")
def then_second_selection_uses_cache(context):
    _assert_success(context)
    assert int(context.host_state.read_text()) == context.first_inspection_attempts


@then("both attention results are identical")
def then_cached_attention_identical(context):
    assert context.payload["summary"]["attention"] == context.first_attention


@given("history inspected under a previous parser identity")
def given_stale_parser_inspections(context):
    init_repo(context)
    context.stale_inspection_shas = [
        commit(context, "zmem(DECISION): stale one"),
        commit(context, "zmem(DECISION): stale two"),
    ]
    _configure_observable_host(context, "counting")
    _query(context)
    _assert_success(context)
    run_svc(context, "stop")
    with database(context) as connection:
        connection.execute("UPDATE inspections SET parser_protocol=2")


@when("the repository is queried under the current parser identity")
def when_current_parser_queries(context):
    commit(context, "", subject="docs: advance parser view")
    _query(context)


@then("every stale inspection is replaced before attention selection")
def then_stale_inspections_replaced(context):
    _assert_success(context)
    with database(context) as connection:
        current = connection.execute("SELECT COUNT(*) FROM inspections WHERE parser_protocol=4").fetchone()[0]
    assert current >= len(context.stale_inspection_shas)


@given("a repository with one supported annotation")
def given_native_one_annotation(context):
    init_repo(context)
    commit(context, "zmem(DECISION): bounded native")


@when("it is queried with no attention override")
def when_native_default_attention(context):
    _query(context)


@then("native attention reports commit limit 500 and node limit 400")
def then_native_default_limits(context):
    _assert_success(context)
    attention = context.payload["summary"]["attention"]
    assert attention["commit_limit"] == 500 and attention["node_limit"] == 400


@then("the complete view reports one selected commit and one annotation")
def then_native_default_usage(context):
    attention = context.payload["summary"]["attention"]
    assert attention["selected_commits"] == 1
    assert attention["selected_nodes"] == 1
    assert attention["truncated"] is False


@given("recent history whose next whole commit would exceed node attention")
def given_native_node_boundary(context):
    init_repo(context)
    context.decision_sha = commit(context, "zmem(DECISION): omitted boundary")
    commit(
        context,
        "\n".join(
            [
                f"zmem(CANCEL)[{context.decision_sha[:8]}, 1]",
                "zmem(UNSUPPORTED): counts too",
            ]
        ),
        subject="chore: recent effects",
    )


@when("it is synchronized with unlimited commits and node limit 2")
def when_native_two_node_attention(context):
    run_svc(
        context,
        "query",
        str(context.repo),
        "--include-invalid",
        "--commit-limit",
        "-1",
        "--node-limit",
        "2",
    )


@then("the boundary commit is excluded in full")
def then_native_boundary_excluded(context):
    _assert_success(context)
    assert context.payload["entries"] == []
    assert context.payload["summary"]["attention"]["selected_commits"] == 1


@then("effects and unsupported annotations count toward the reached node bound")
def then_native_effects_count(context):
    attention = context.payload["summary"]["attention"]
    assert attention["selected_nodes"] == 2
    assert attention["reached"] == ["node"]


@given("environmental commit and node limits of one")
def given_native_environment_one(context):
    init_repo(context)
    for index in range(3):
        commit(context, f"zmem(DECISION): native {index}")
    context.env["ZMEM_COMMIT_LIMIT"] = "1"
    context.env["ZMEM_NODE_LIMIT"] = "1"


@when("a query explicitly requests commit limit 3 and node limit 2")
def when_native_explicit_limits(context):
    run_svc(
        context,
        "query",
        str(context.repo),
        "--include-invalid",
        "--commit-limit",
        "3",
        "--node-limit",
        "2",
    )


@then("native attention reports commit limit 3 and node limit 2")
def then_native_explicit_limits(context):
    _assert_success(context)
    attention = context.payload["summary"]["attention"]
    assert attention["commit_limit"] == 3 and attention["node_limit"] == 2


@when("it is queried with node limit zero")
def when_native_invalid_node(context):
    run_svc(context, "query", str(context.repo), "--node-limit", "0")


@then("a structured request failure identifies node limit")
def then_native_invalid_node(context):
    assert context.completed.returncode != 0
    assert "node limit" in context.completed.stderr


@given("a repository anchored with an older decision outside its bounded view")
def given_bounded_anchor_omits_decision(context):
    init_repo(context)
    context.decision_sha = commit(context, "zmem(DECISION): older bounded")
    commit(context, "zmem(LESSON_LEARNT): newest bounded")
    run_svc(
        context,
        "query",
        str(context.repo),
        "--include-invalid",
        "--commit-limit",
        "1",
        "--node-limit",
        "1",
    )
    _assert_success(context)
    assert all(row["sha"] != context.decision_sha for row in context.payload["entries"])


@when("the repository is queried with both attention limits unlimited")
def when_query_unlimited_attention(context):
    run_svc(
        context,
        "query",
        str(context.repo),
        "--include-invalid",
        "--commit-limit",
        "-1",
        "--node-limit",
        "-1",
    )


@then("complete history is rebuilt and the older decision is returned")
def then_unlimited_rebuild_returns_old(context):
    _assert_success(context)
    assert any(row["sha"] == context.decision_sha for row in context.payload["entries"])


@then("the anchor reports a complete attention identity")
def then_anchor_complete_attention(context):
    identity = context.payload["summary"]["trail"]["attention_identity"]
    assert identity.startswith("v1:-1:-1:") and ":false:" in identity


def _given_proposed_cancel_before_view(context):
    init_repo(context)
    context.decision_sha = commit(context, "zmem(DECISION): deep bounded target")
    commit(context, "zmem(LESSON_LEARNT): newer attention")
    context.proposed_message = f"fix: cancel\n\nzmem(CANCEL)[{context.decision_sha[:8]}, 1]"
    _query(context)
    _assert_success(context)
    context.persistent_before = _persistent_state(context)


@given("a proposed cancellation whose decision precedes its attention view")
def given_cancel_before_attention(context):
    _given_proposed_cancel_before_view(context)


@when("I deep-check it under that bounded attention policy")
def when_deep_check_bounded(context):
    run_svc(
        context,
        "check",
        str(context.repo),
        "--deep",
        "--commit-limit",
        "-1",
        "--node-limit",
        "1",
        input_text=context.proposed_message,
    )


@then("the effect is unsuccessful because history is incomplete")
def then_deep_effect_incomplete(context):
    _assert_success(context)
    assert context.payload["ok"] is False
    assert context.payload["attention"]["reached"] == ["node"]
    assert any("attention threshold reached" in item for item in context.payload["diagnostics"])


@then("persistent state remains unchanged")
def then_attention_check_persistent_unchanged(context):
    _assert_persistent_state(context)


@given("a proposed cancellation whose decision precedes its default attention view")
def given_cancel_before_default_attention(context):
    _given_proposed_cancel_before_view(context)


@when("I deep-check it with both attention limits unlimited")
def when_deep_check_unlimited(context):
    run_svc(
        context,
        "check",
        str(context.repo),
        "--deep",
        "--commit-limit",
        "-1",
        "--node-limit",
        "-1",
        input_text=context.proposed_message,
    )


@then("complete replay reports cancellation from valid to invalid")
def then_deep_unlimited_cancel(context):
    _assert_success(context)
    effect = context.payload["effects"][0]
    assert effect["before_valid"] is True and effect["after_valid"] is False
    assert context.payload["attention"]["truncated"] is False


def _git(context, *args: str) -> str:
    completed = subprocess.run(
        ["git", "-C", context.repo, *args],
        env=context.env,
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 0, (completed.stdout, completed.stderr)
    return completed.stdout.strip()


def _query_selector(context, selector: str, observed: str, *, node_limit: int | None = None) -> dict | None:
    args = [
        "query",
        str(context.repo),
        "--include-invalid",
        "--ref",
        selector,
        "--observed-oid",
        observed,
    ]
    if node_limit is not None:
        args.extend(("--node-limit", str(node_limit)))
    run_svc(context, *args)
    return context.payload


@given("two local branches at one commit under identical trail identities")
def given_two_names_one_commit(context):
    init_repo(context)
    context.shared_head = commit(context, "zmem(DECISION): shared trail")
    _git(context, "branch", "alpha", context.shared_head)
    _git(context, "branch", "beta", context.shared_head)


@when("both branches are queried")
def when_query_two_names(context):
    context.trail_results = [_query_selector(context, selector, context.shared_head) for selector in ("alpha", "beta")]
    assert all(result is not None for result in context.trail_results)


@then("both selectors reuse one immutable trail")
def then_two_names_one_trail(context):
    trails = [result["summary"]["trail"] for result in context.trail_results]
    assert trails[0]["trail_id"] == trails[1]["trail_id"]
    assert [trail["requested_selector"] for trail in trails] == ["alpha", "beta"]
    with database(context) as connection:
        assert connection.execute("SELECT COUNT(*) FROM trails").fetchone()[0] == 1


@given("a cached local branch alias whose branch has moved")
def given_stale_branch_alias(context):
    init_repo(context)
    context.old_head = commit(context, "zmem(DECISION): old branch state")
    _git(context, "branch", "moving", context.old_head)
    first = _query_selector(context, "moving", context.old_head)
    assert first is not None
    context.old_trail = first["summary"]["trail"]["trail_id"]
    _git(context, "switch", "moving")
    context.new_head = commit(context, "zmem(LESSON_LEARNT): new branch state")


@when("the branch is queried using its live commit identity")
def when_query_live_branch(context):
    context.live_result = _query_selector(context, "moving", context.new_head)
    _assert_success(context)


@then("the stale alias is ignored and the live trail is returned")
def then_stale_alias_ignored(context):
    trail = context.live_result["summary"]["trail"]
    assert trail["resolved_oid"] == context.new_head and trail["trail_id"] != context.old_trail
    with database(context) as connection:
        alias = connection.execute("SELECT trail_id,resolved_oid FROM ref_aliases WHERE selector='moving'").fetchone()
    assert alias == (trail["trail_id"], context.new_head)


@given("a ref that moves after the client observes its commit")
def given_ref_race(context):
    init_repo(context)
    context.observed_head = commit(context, "zmem(DECISION): observed")
    context.branch = _git(context, "branch", "--show-current")
    context.live_head = commit(context, "zmem(LESSON_LEARNT): moved")


@when("the service resolves that ref for the request")
def when_resolve_stale_ref(context):
    _query_selector(context, context.branch, context.observed_head)


@then("the request fails without publishing or advancing a trail")
def then_stale_ref_atomic(context):
    assert context.completed.returncode != 0
    assert "stale ref" in context.completed.stderr
    db_path = context.home / "db" / "entries.db"
    if db_path.exists():
        with sqlite3.connect(db_path) as connection:
            assert connection.execute("SELECT COUNT(*) FROM trails").fetchone()[0] == 0


@given("two trails sharing a decision while only one reaches its cancellation")
def given_branch_local_cancel(context):
    init_repo(context)
    context.decision_sha = commit(context, "zmem(DECISION): branch-local validity")
    _git(context, "branch", "valid", context.decision_sha)
    _git(context, "switch", "-c", "cancelled")
    context.cancelled_head = commit(context, f"zmem(CANCEL)[{context.decision_sha[:10]}, 1]")


@when("both trails are queried including invalid entries")
def when_query_cancelled_and_valid(context):
    context.cancelled_result = _query_selector(context, "cancelled", context.cancelled_head)
    context.valid_result = _query_selector(context, "valid", context.decision_sha)
    assert context.cancelled_result is not None and context.valid_result is not None


@then("the shared decision is invalid only on the cancellation trail")
def then_cancel_is_trail_local(context):
    cancelled = context.cancelled_result["entries"][0]
    valid = context.valid_result["entries"][0]
    assert cancelled["sha"] == valid["sha"] == context.decision_sha
    assert cancelled["valid"] is False and valid["valid"] is True


@given("a candidate trail containing an incomplete META range")
def given_incomplete_meta_trail(context):
    init_repo(context)
    context.target_sha = commit(context, "zmem(DECISION): unchanged target")
    context.meta_head = commit(
        context,
        f"zmem(META)[{context.target_sha[:10]}, {context.target_sha[:10]}, owner=team]",
    )
    context.branch = _git(context, "branch", "--show-current")


@when("that trail is constructed")
def when_construct_incomplete_meta(context):
    _query_selector(context, context.branch, context.meta_head, node_limit=1)


@then("neither the candidate trail nor partial metadata becomes visible")
def then_incomplete_meta_is_atomic(context):
    assert context.completed.returncode != 0
    assert "complete" in context.completed.stderr.lower()
    with database(context) as connection:
        assert connection.execute("SELECT COUNT(*) FROM trails").fetchone()[0] == 0
        assert connection.execute("SELECT COUNT(*) FROM metadata_assignments").fetchone()[0] == 0


@given("a populated schema-three cache with a materialized projection")
def given_schema_three_projection(context):
    init_repo(context)
    context.legacy_oid = commit(context, "zmem(DECISION): legacy")
    db_path = context.home / "db" / "entries.db"
    db_path.parent.mkdir(parents=True)
    with sqlite3.connect(db_path) as connection:
        connection.executescript(
            """
            PRAGMA foreign_keys=ON;
            PRAGMA user_version=3;
            CREATE TABLE repositories(id INTEGER PRIMARY KEY,path TEXT NOT NULL UNIQUE,trusted_extensions INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE anchors(repository_id INTEGER PRIMARY KEY REFERENCES repositories(id) ON DELETE CASCADE,head TEXT NOT NULL,schema_version INTEGER NOT NULL,extension_hash TEXT NOT NULL,attention_identity TEXT NOT NULL);
            CREATE TABLE commits(repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,oid TEXT NOT NULL,commit_time INTEGER NOT NULL,message TEXT NOT NULL,PRIMARY KEY(repository_id,oid));
            CREATE TABLE entries(repository_id INTEGER NOT NULL,commit_oid TEXT NOT NULL,annotation_index INTEGER NOT NULL,entry_type TEXT NOT NULL,content TEXT NOT NULL,score REAL NOT NULL,valid INTEGER NOT NULL,commit_time INTEGER NOT NULL DEFAULT 0,scope TEXT,PRIMARY KEY(repository_id,commit_oid,annotation_index),FOREIGN KEY(repository_id,commit_oid) REFERENCES commits(repository_id,oid) ON DELETE CASCADE);
            CREATE TABLE relationships(repository_id INTEGER NOT NULL,commit_oid TEXT NOT NULL,source TEXT NOT NULL,target TEXT NOT NULL,score REAL NOT NULL,FOREIGN KEY(repository_id,commit_oid) REFERENCES commits(repository_id,oid) ON DELETE CASCADE);
            CREATE TABLE diagnostics(repository_id INTEGER NOT NULL,commit_oid TEXT NOT NULL,message TEXT NOT NULL,FOREIGN KEY(repository_id,commit_oid) REFERENCES commits(repository_id,oid) ON DELETE CASCADE);
            CREATE TABLE inspections(commit_oid TEXT NOT NULL,parser_protocol INTEGER NOT NULL,annotation_count INTEGER NOT NULL,parser_diagnostics TEXT NOT NULL,PRIMARY KEY(commit_oid,parser_protocol));
            """
        )
        connection.execute("INSERT INTO repositories VALUES(1,?1,0)", [str(context.repo.resolve())])
        connection.execute("INSERT INTO commits VALUES(1,?1,10,'zmem(DECISION): legacy')", [context.legacy_oid])
        connection.execute(
            "INSERT INTO entries VALUES(1,?1,1,'DECISION','legacy',1.0,1,10,NULL)",
            [context.legacy_oid],
        )
        connection.execute(
            "INSERT INTO anchors VALUES(1,?1,3,'legacy-extension','legacy-attention')",
            [context.legacy_oid],
        )


@when("the compatible service opens the database")
def when_service_migrates_cache(context):
    run_svc(context, "query", str(context.repo), "--include-invalid")
    _assert_success(context)


@then("a legacy trail preserves its query state without Git replay")
def then_legacy_trail_preserved(context):
    assert context.payload["summary"]["indexed_commits"] == 0
    assert context.payload["entries"][0]["content"] == "legacy"
    assert context.payload["entries"][0]["affected_areas"] is None
    with database(context) as connection:
        assert connection.execute("PRAGMA user_version").fetchone()[0] == 4
        assert connection.execute("SELECT legacy FROM trails").fetchone()[0] == 1


@given("memory reachable from a detached commit identity")
def given_detached_memory(context):
    init_repo(context)
    context.detached_oid = commit(context, "zmem(DECISION): detached")


@when("that commit is queried directly")
def when_query_detached_oid(context):
    context.detached_result = _query_selector(context, context.detached_oid, context.detached_oid)
    _assert_success(context)


@then("the response identifies its immutable trail without a local branch alias")
def then_detached_has_no_alias(context):
    trail = context.detached_result["summary"]["trail"]
    assert trail["requested_selector"] == context.detached_oid
    assert trail["resolved_oid"] == context.detached_oid
    with database(context) as connection:
        assert connection.execute("SELECT COUNT(*) FROM ref_aliases").fetchone()[0] == 0


def _commit_files(context, body: str, files: dict[str, str], *, subject: str = "feat: paths") -> str:
    for relative, content in files.items():
        path = context.repo / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
    _git(context, "add", "-A")
    args = ["commit", "-q", "-m", subject]
    if body:
        args.extend(("-m", body))
    _git(context, *args)
    return _git(context, "rev-parse", "HEAD")


@given("two trails sharing an entry and only one containing a META owner assignment")
def given_branch_owner_override(context):
    init_repo(context)
    context.owner_target = commit(context, "zmem(DECISION): shared owner target")
    _git(context, "branch", "plain", context.owner_target)
    _git(context, "switch", "-c", "owned")
    context.owned_head = commit(
        context,
        f"zmem(META)[{context.owner_target[:10]}, {context.owner_target[:10]}, owner=platform]",
    )


@when("both trails are queried")
def when_query_owner_trails(context):
    context.owned_result = _query_selector(context, "owned", context.owned_head)
    context.plain_result = _query_selector(context, "plain", context.owner_target)
    assert context.owned_result is not None and context.plain_result is not None


@then("only the META-containing trail reports the assigned owner")
def then_owner_is_trail_local(context):
    assert context.owned_result["entries"][0]["owner"] == "platform"
    assert context.plain_result["entries"][0]["owner"] is None


@given("a new commit renaming a file from a/old to b/sub/new")
def given_cross_area_rename(context):
    init_repo(context)
    _commit_files(context, "", {"a/old": "old\n"})
    (context.repo / "b" / "sub").mkdir(parents=True)
    _git(context, "mv", "a/old", "b/sub/new")
    _git(context, "commit", "-q", "-m", "feat: rename", "-m", "zmem(DECISION): renamed")
    context.rename_head = _git(context, "rev-parse", "HEAD")


@when("its shared commit fact enters the cache")
def when_index_renamed_fact(context):
    head = getattr(context, "rename_head", None) or context.broad_head
    run_svc(context, "query", str(context.repo), "--include-invalid", "--observed-oid", head)
    _assert_success(context)


@then("affected-area derivation includes a and b/sub")
def then_rename_has_both_areas(context):
    entry = next(row for row in context.payload["entries"] if row["sha"] == context.rename_head)
    assert entry["affected_areas"] == ["a", "b/sub"]


@given("a new commit whose compact derivation has four areas")
def given_four_area_commit(context):
    init_repo(context)
    context.broad_head = _commit_files(
        context,
        "zmem(DECISION): broad change",
        {"a/x": "a", "b/x": "b", "c/x": "c", "d/x": "d"},
    )


@then("its affected areas are null and globally applicable")
def then_broad_fact_is_global(context):
    entry = next(row for row in context.payload["entries"] if row["sha"] == context.broad_head)
    assert entry["affected_areas"] is None


@given("a migrated legacy entry without an affected-area override")
def given_migrated_global_entry(context):
    given_schema_three_projection(context)
    when_service_migrates_cache(context)


@when("the trail is queried with any affected-area filter")
def when_query_migrated_area(context):
    zmem = ZMEM_ROOT / ".venv" / ("Scripts" if os.name == "nt" else "bin") / "zmem"
    context.env["ZMEM_SVC"] = str(context.svc)
    context.completed = subprocess.run(
        [zmem, "--repo", context.repo, "recall", "--area", "some/subtree"],
        env=context.env,
        capture_output=True,
        text=True,
        check=False,
    )
    context.payload = json.loads(context.completed.stdout)


@then("the entry reports null affected areas and remains visible")
def then_migrated_entry_matches_area(context):
    _assert_success(context)
    assert context.payload["count"] == 1
    assert context.payload["results"][0]["affected_areas"] is None


@given("a trail with metadata targets in a complete range")
def given_complete_metadata_target(context):
    init_repo(context)
    context.scalar_target = commit(context, "zmem(DECISION): scalar target")


@when("META attempts to append to scalar owner")
def when_meta_appends_scalar(context):
    context.invalid_meta_head = commit(
        context,
        f"zmem(META)[{context.scalar_target[:10]}, {context.scalar_target[:10]}, owner+=team]",
    )
    run_svc(context, "query", str(context.repo), "--include-invalid", "--observed-oid", context.invalid_meta_head)
    _assert_success(context)


@then("no target metadata changes and the invalid operation is diagnosed")
def then_scalar_append_diagnosed(context):
    target = next(row for row in context.payload["entries"] if row["sha"] == context.scalar_target)
    assert target["owner"] is None
    assert any("invalid META" in diagnostic["message"] for diagnostic in context.payload["diagnostics"])


@given("a META range spanning commits on a merged branch")
def given_merged_meta_range(context):
    init_repo(context)
    context.range_from = commit(context, "zmem(DECISION): range base")
    base_branch = _git(context, "branch", "--show-current")
    _git(context, "switch", "-c", "side")
    context.side_entry = _commit_files(
        context,
        "zmem(LESSON_LEARNT): side entry",
        {"side/memory.txt": "side\n"},
    )
    _git(context, "switch", base_branch)
    context.main_entry = _commit_files(
        context,
        "zmem(LESSON_LEARNT): main entry",
        {"main/memory.txt": "main\n"},
    )
    _git(context, "merge", "--no-ff", "-m", "merge side", "side")
    context.range_to = _git(context, "rev-parse", "HEAD")
    context.range_head = commit(
        context,
        f"zmem(META)[{context.range_from[:10]}, {context.range_to[:10]}, owner=merged]",
    )


@when("the selected trail applies the metadata patch")
def when_apply_merged_range(context):
    run_svc(context, "query", str(context.repo), "--include-invalid", "--observed-oid", context.range_head)
    _assert_success(context)


@then("every qualifying descendant and ancestor in the inclusive range is patched")
def then_merged_range_is_inclusive(context):
    by_sha = {row["sha"]: row for row in context.payload["entries"]}
    for oid in (context.range_from, context.side_entry, context.main_entry):
        assert by_sha[oid]["owner"] == "merged"


@given("concurrent META assignments conflict before a merge")
def given_concurrent_metadata(context):
    init_repo(context)
    context.conflict_target = commit(context, "zmem(DECISION): conflict target")
    base_branch = _git(context, "branch", "--show-current")
    _git(context, "switch", "-c", "owner-b")
    context.owner_b = _commit_files(
        context,
        f"zmem(META)[{context.conflict_target[:10]}, {context.conflict_target[:10]}, owner=beta]",
        {"owner-b.txt": "beta\n"},
    )
    _git(context, "switch", base_branch)
    context.owner_a = _commit_files(
        context,
        f"zmem(META)[{context.conflict_target[:10]}, {context.conflict_target[:10]}, owner=alpha]",
        {"owner-a.txt": "alpha\n"},
    )
    _git(context, "merge", "--no-ff", "-m", "merge owners", "owner-b")
    context.conflict_merge = _git(context, "rev-parse", "HEAD")
    run_svc(context, "query", str(context.repo), "--include-invalid", "--observed-oid", context.conflict_merge)
    _assert_success(context)
    target = next(row for row in context.payload["entries"] if row["sha"] == context.conflict_target)
    assert target["owner"] is None and target["metadata_conflicts"] == ["owner"]


@when("a descendant META assigns that key after the merge")
def when_descendant_resolves_metadata(context):
    context.resolved_head = commit(
        context,
        f"zmem(META)[{context.conflict_target[:10]}, {context.conflict_target[:10]}, owner=resolved]",
    )
    run_svc(context, "query", str(context.repo), "--include-invalid", "--observed-oid", context.resolved_head)
    _assert_success(context)


@then("the descendant value is reported and the conflict is cleared")
def then_metadata_conflict_cleared(context):
    target = next(row for row in context.payload["entries"] if row["sha"] == context.conflict_target)
    assert target["owner"] == "resolved" and target["metadata_conflicts"] == []


@given("a selected commit with a supported entry, unsupported annotation, and valid effect")
def given_mixed_trail_annotations(context):
    init_repo(context)
    context.mixed_target = commit(context, "zmem(DECISION): mixed target")
    context.mixed_head = commit(
        context,
        "\n".join(
            (
                "zmem(LESSON_LEARNT): supported",
                "zmem(UNKNOWN): unsupported",
                f"zmem(DECAY)[{context.mixed_target[:10]}, 1, 0.5]",
            )
        ),
    )


@when("its immutable trail is indexed")
def when_index_mixed_trail(context):
    run_svc(context, "query", str(context.repo), "--include-invalid", "--observed-oid", context.mixed_head)
    _assert_success(context)


@then("all annotations consume attention, only the entry consumes capacity, and the effect updates trail state")
def then_mixed_trail_counts(context):
    assert context.payload["summary"]["attention"]["selected_nodes"] == 4
    assert len(context.payload["entries"]) == 2
    target = next(row for row in context.payload["entries"] if row["sha"] == context.mixed_target)
    assert target["score"] == 0.5
    assert any("unsupported" in row["message"].lower() for row in context.payload["diagnostics"])


@given("a retained trail whose branch advances by two commits")
def given_advancing_retained_trail(context):
    init_repo(context)
    context.advance_base = commit(context, "zmem(DECISION): shared base")
    context.advance_branch = _git(context, "branch", "--show-current")
    first = _query_selector(context, context.advance_branch, context.advance_base)
    assert first is not None
    context.advance_old_trail = first["summary"]["trail"]["trail_id"]
    commit(context, "zmem(LESSON_LEARNT): advance one")
    context.advance_head = commit(context, "zmem(LESSON_LEARNT): advance two")


@when("the advanced branch is queried under the same compatible view")
def when_query_advanced_trail(context):
    context.advance_result = _query_selector(context, context.advance_branch, context.advance_head)
    _assert_success(context)


@then("a distinct trail reuses prior shared facts and indexes the new commits")
def then_advanced_trail_reuses_facts(context):
    trail = context.advance_result["summary"]["trail"]
    assert trail["trail_id"] != context.advance_old_trail
    assert context.advance_result["summary"]["indexed_commits"] == 2
    with database(context) as connection:
        assert connection.execute("SELECT COUNT(*) FROM expansion_facts").fetchone()[0] == 3


@given("a retained trail reaching a cancellation and a rewritten branch without it")
def given_rewritten_cancel_trail(context):
    init_repo(context)
    context.rewrite_target = commit(context, "zmem(DECISION): rewrite target")
    context.rewrite_branch = _git(context, "branch", "--show-current")
    context.cancel_head = commit(context, f"zmem(CANCEL)[{context.rewrite_target[:10]}, 1]")
    cancelled = _query_selector(context, context.rewrite_branch, context.cancel_head)
    assert cancelled is not None and cancelled["entries"][0]["valid"] is False
    context.cancel_trail = cancelled["summary"]["trail"]["trail_id"]
    _git(context, "reset", "--hard", context.rewrite_target)
    context.rewritten_head = _commit_files(
        context,
        "zmem(LESSON_LEARNT): replacement",
        {"replacement.txt": "replacement\n"},
    )


@when("the rewritten branch is queried")
def when_query_rewritten_branch(context):
    context.rewritten_result = _query_selector(context, context.rewrite_branch, context.rewritten_head)
    _assert_success(context)
    context.former_result = _query_selector(context, context.cancel_head, context.cancel_head)
    _assert_success(context)


@then("its new trail reports the uncancelled state while the former trail stays immutable")
def then_rewrite_keeps_old_trail(context):
    current = next(row for row in context.rewritten_result["entries"] if row["sha"] == context.rewrite_target)
    former = next(row for row in context.former_result["entries"] if row["sha"] == context.rewrite_target)
    assert current["valid"] is True and former["valid"] is False
    assert context.former_result["summary"]["trail"]["trail_id"] == context.cancel_trail


@given("only a default-bounded trail for a repository")
def given_default_bounded_trail(context):
    init_repo(context)
    annotations = "\n".join(f"zmem(LESSON_LEARNT): node {index}" for index in range(401))
    context.large_head = commit(context, annotations)
    run_svc(context, "query", str(context.repo), "--include-invalid", "--observed-oid", context.large_head)
    _assert_success(context)
    context.bounded_summary = context.payload["summary"]
    assert context.bounded_summary["attention"]["truncated"] is True


@when("the repository is queried with unlimited commit and node attention")
def when_query_unlimited_trail(context):
    run_svc(
        context,
        "query",
        str(context.repo),
        "--include-invalid",
        "--observed-oid",
        context.large_head,
        "--commit-limit",
        "-1",
        "--node-limit",
        "-1",
    )
    _assert_success(context)


@then("a complete-history trail is constructed instead of reusing the bounded view as complete")
def then_unlimited_trail_is_distinct(context):
    assert context.payload["summary"]["attention"]["truncated"] is False
    assert context.payload["summary"]["trail"]["trail_id"] != context.bounded_summary["trail"]["trail_id"]
    assert len(context.payload["entries"]) == 401


@given("a candidate trail with a fatal effect failure")
def given_fatal_effect_trail(context):
    given_incomplete_meta_trail(context)


@when("indexing reaches that effect")
def when_index_fatal_effect(context):
    when_construct_incomplete_meta(context)


@then("neither partial target state nor a partial trail is visible")
def then_fatal_effect_is_atomic(context):
    then_incomplete_meta_is_atomic(context)


@given("an extension host reporting an incompatible trail protocol")
def given_trail_protocol_mismatch(context):
    given_bad_protocol_host(context)
    with database(context) as connection:
        context.protocol_fact_count = connection.execute("SELECT COUNT(*) FROM expansion_facts").fetchone()[0]
        context.protocol_trail_count = connection.execute("SELECT COUNT(*) FROM trails").fetchone()[0]


@when("the service requests expansion for a candidate trail")
def when_request_bad_protocol_trail(context):
    _query(context)


@then("construction fails without publishing shared facts or trail state")
def then_bad_protocol_publishes_nothing(context):
    assert context.completed.returncode != 0
    with database(context) as connection:
        assert connection.execute("SELECT COUNT(*) FROM expansion_facts").fetchone()[0] == context.protocol_fact_count
        assert connection.execute("SELECT COUNT(*) FROM trails").fetchone()[0] == context.protocol_trail_count


@given("a host journal containing a validated ordered metadata-patch action")
def given_metadata_patch_journal(context):
    init_repo(context)
    context.journal_target = commit(context, "zmem(DECISION): journal target")
    context.journal_head = commit(
        context,
        f"zmem(META)[{context.journal_target[:10]}, {context.journal_target[:10]}, owner=journal]",
    )


@when("the service constructs its selected trail")
def when_construct_metadata_journal_trail(context):
    run_svc(context, "query", str(context.repo), "--include-invalid", "--observed-oid", context.journal_head)
    _assert_success(context)


@then("the service validates and atomically applies the complete metadata range")
def then_metadata_journal_applied(context):
    target = next(row for row in context.payload["entries"] if row["sha"] == context.journal_target)
    assert target["owner"] == "journal"
    with database(context) as connection:
        assert connection.execute("SELECT COUNT(*) FROM trail_metadata WHERE owner='journal'").fetchone()[0] == 1


@given("a host response containing data absent from its validated action journal")
def given_bypass_host_response(context):
    init_repo(context)
    commit(context, "zmem(DECISION): bypass attempt")
    _configure_observable_host(context, "bypass")


@then("it rejects the response without publishing the candidate trail")
def then_bypass_response_rejected(context):
    assert context.completed.returncode != 0
    with database(context) as connection:
        assert connection.execute("SELECT COUNT(*) FROM trails").fetchone()[0] == 0


@given("a registered repository whose observed HEAD has advanced")
def given_observed_advanced_head(context):
    given_advanced_repo(context)


@when("that exact observed commit is queried")
def when_query_exact_observed_head(context):
    run_svc(context, "query", str(context.repo), "--include-invalid", "--observed-oid", context.head)
    _assert_success(context)


@then("the service returns an immutable trail through that commit")
def then_exact_head_trail(context):
    assert context.payload["summary"]["trail"]["resolved_oid"] == context.head
    assert context.payload["summary"]["trail"]["trail_id"]


@given("a resolvable tag, branch, or commit that is not checked out")
def given_unchecked_selector(context):
    init_repo(context)
    context.unchecked_oid = commit(context, "zmem(DECISION): unchecked")
    _git(context, "branch", "unoccupied", context.unchecked_oid)
    commit(context, "zmem(LESSON_LEARNT): checked out")
    context.worktree_before = _git(context, "status", "--porcelain=v1", "--branch")


@when("the selector and observed identity are queried")
def when_query_unchecked_selector(context):
    context.unchecked_result = _query_selector(context, "unoccupied", context.unchecked_oid)
    _assert_success(context)


@then("the compatible trail is returned without modifying the worktree")
def then_unchecked_selector_does_not_checkout(context):
    assert context.unchecked_result["summary"]["trail"]["resolved_oid"] == context.unchecked_oid
    assert _git(context, "status", "--porcelain=v1", "--branch") == context.worktree_before


@given("capacity is exceeded by old unreferenced trail state sharing commit facts")
def given_unreferenced_trail_capacity(context):
    write_config(context, max_entries=2, protect_recent_days=0)
    init_repo(context)
    context.retention_base = commit(
        context,
        "zmem(DECISION): retained shared fact",
        timestamp="2000-01-01T00:00:00+00:00",
    )
    main = _git(context, "branch", "--show-current")
    _git(context, "branch", "secondary", context.retention_base)
    first = _query_selector(context, main, context.retention_base)
    assert first is not None
    context.unreferenced_trail = first["summary"]["trail"]["trail_id"]
    context.main_head = commit(
        context,
        "zmem(LESSON_LEARNT): main retained",
        timestamp="2001-01-01T00:00:00+00:00",
    )
    _query_selector(context, main, context.main_head)
    _git(context, "switch", "secondary")
    context.secondary_head = _commit_files(
        context,
        "zmem(LESSON_LEARNT): secondary retained",
        {"secondary.txt": "secondary\n"},
    )


@when("retention runs after a write")
def when_retention_runs_after_write(context):
    context.retention_result = _query_selector(context, "secondary", context.secondary_head)
    _assert_success(context)


@then("unreferenced trail state is evicted before facts used by a retained trail")
def then_unreferenced_trail_first(context):
    with database(context) as connection:
        assert (
            connection.execute("SELECT COUNT(*) FROM trails WHERE id=?1", [context.unreferenced_trail]).fetchone()[0]
            == 0
        )
        assert (
            connection.execute("SELECT COUNT(*) FROM commits WHERE oid=?1", [context.retention_base]).fetchone()[0] == 1
        )
        assert (
            connection.execute(
                "SELECT COUNT(*) FROM expansion_facts WHERE commit_oid=?1", [context.retention_base]
            ).fetchone()[0]
            == 1
        )


@given("an old shared commit fact reused by a new trail")
def given_old_fact_reused(context):
    init_repo(context)
    context.old_fact = commit(
        context,
        "zmem(DECISION): old source fact",
        timestamp="2000-01-01T00:00:00+00:00",
    )
    context.old_source_time = int(_git(context, "show", "-s", "--format=%ct", context.old_fact))
    branch = _git(context, "branch", "--show-current")
    _query_selector(context, branch, context.old_fact)
    context.new_fact_head = commit(
        context,
        "zmem(LESSON_LEARNT): new trail",
        timestamp="2020-01-01T00:00:00+00:00",
    )


@when("retention orders eligible shared facts")
def when_retention_orders_reused_fact(context):
    branch = _git(context, "branch", "--show-current")
    _query_selector(context, branch, context.new_fact_head)
    _assert_success(context)


@then("reuse does not make the fact newer than its source commit time")
def then_reuse_keeps_source_time(context):
    with database(context) as connection:
        stored = connection.execute("SELECT commit_time FROM commits WHERE oid=?1", [context.old_fact]).fetchone()[0]
        facts = connection.execute(
            "SELECT COUNT(*) FROM expansion_facts WHERE commit_oid=?1", [context.old_fact]
        ).fetchone()[0]
    assert stored == context.old_source_time and facts == 1


@given("overlapping facts in a retained trail and an old unreferenced trail")
def given_overlapping_retained_facts(context):
    write_config(context, max_entries=2, protect_recent_days=0)
    init_repo(context)
    context.overlap_target = commit(
        context,
        "zmem(DECISION): overlap target",
        timestamp="2000-01-01T00:00:00+00:00",
    )
    main = _git(context, "branch", "--show-current")
    _git(context, "branch", "secondary", context.overlap_target)
    base = _query_selector(context, main, context.overlap_target)
    assert base is not None
    context.overlap_old_trail = base["summary"]["trail"]["trail_id"]
    context.decay_head = commit(
        context,
        f"zmem(DECAY)[{context.overlap_target[:10]}, 1, 0.5]",
        timestamp="2001-01-01T00:00:00+00:00",
    )
    retained = _query_selector(context, main, context.decay_head)
    assert retained is not None and retained["entries"][0]["score"] == 0.5
    context.retained_selector = main
    context.retained_trail = retained["summary"]["trail"]["trail_id"]
    _git(context, "switch", "secondary")
    context.secondary_overlap_head = _commit_files(
        context,
        "zmem(LESSON_LEARNT): secondary overlap",
        {"secondary-overlap.txt": "secondary\n"},
    )


@when("the old trail is evicted and the retained trail is queried")
def when_evict_old_and_query_retained(context):
    _query_selector(context, "secondary", context.secondary_overlap_head)
    context.retained_again = _query_selector(context, context.retained_selector, context.decay_head)
    _assert_success(context)


@then("the retained state is reused without duplicate effect application")
def then_retained_effect_not_duplicated(context):
    assert context.retained_again["summary"]["trail"]["trail_id"] == context.retained_trail
    target = next(row for row in context.retained_again["entries"] if row["sha"] == context.overlap_target)
    assert target["score"] == 0.5
    with database(context) as connection:
        assert (
            connection.execute("SELECT COUNT(*) FROM trails WHERE id=?1", [context.overlap_old_trail]).fetchone()[0]
            == 0
        )
