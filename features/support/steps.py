from __future__ import annotations

import json
import os
import sqlite3
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from behave import given, then, when

from features.support.lifecycle import (
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


@given("an indexed repository with one valid decision")
def given_check_valid_decision(context):
    init_repo(context)
    context.decision_sha = commit(context, "zmem(DECISION): preview target")
    run_svc(context, "add", str(context.repo))
    _assert_success(context)
    context.anchor_before = context.payload["head"]


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


@then("the stored entry and anchor remain unchanged after the check")
def then_store_unchanged_after_check(context):
    assert _entry(context, context.decision_sha)["valid"] is True
    with database(context) as connection:
        anchor = connection.execute("SELECT head FROM anchors").fetchone()[0]
        virtual = connection.execute("SELECT COUNT(*) FROM commits WHERE oid=?1", ["0" * 40]).fetchone()[0]
    assert anchor == context.anchor_before and virtual == 0


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
    write_config(context, max_entries=1, protect_recent_days=0)
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
            "SELECT a.head, r.trusted_extensions FROM anchors a JOIN repositories r ON r.id=a.repository_id"
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
    assert context.payload["protocol_version"] == 2


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
    with database(context) as connection:
        anchor = connection.execute("SELECT head FROM anchors").fetchone()[0]
    assert anchor == context.head


@given("an indexed history that cancels a decision")
def given_cancelled_history(context):
    init_repo(context)
    context.decision_head = commit(context, "zmem(DECISION): keep")
    context.cancel_head = commit(context, f"zmem(CANCEL)[{context.decision_head[:8]}, 1]")
    _query(context)
    _assert_success(context)
    assert context.payload["entries"][0]["valid"] is False


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
    assert context.payload["summary"]["indexed_commits"] == 2


@given("indexing fails fatally after proposing an effect")
def given_failing_range(context):
    init_repo(context)
    context.base = commit(context, "zmem(DECISION): stable")
    run_svc(context, "add", str(context.repo))
    _assert_success(context)
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
        anchor = connection.execute("SELECT head FROM anchors").fetchone()[0]
        score = connection.execute("SELECT score FROM entries WHERE content='stable'").fetchone()[0]
    assert anchor == context.base and score == 1.0


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
        {"protocol_version": 2, "extension_hash": "x", "entries": [{"content": "bypass"}]}
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
        anchor = connection.execute("SELECT head FROM anchors").fetchone()[0]
    assert anchor == context.anchor


@given("an anchor containing the previous extension-set identity")
def given_previous_identity(context):
    context.extension = write_global_extension(
        context,
        "expanders",
        "custom.py",
        "API_VERSION=1\nVALUE=1\ndef register(registry, mode='extend'): pass\n",
    )
    init_repo(context)
    commit(context, "zmem(DECISION): stable")
    _query(context)
    _assert_success(context)


@when("the current extension host reports a different identity")
def when_identity_changes(context):
    context.extension.write_text("API_VERSION=1\nVALUE=2\ndef register(registry, mode='extend'): pass\n")
    _query(context)


@then("repository synchronization selects a rebuild")
def then_rebuild_selected(context):
    _assert_success(context)
    assert context.payload["summary"]["indexed_commits"] == 1
    assert len(context.payload["entries"]) == 1


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
    with database(context) as connection:
        identity = connection.execute("SELECT attention_identity FROM anchors").fetchone()[0]
    assert identity.startswith("v1:-1:-1:") and ":false:" in identity


def _given_proposed_cancel_before_view(context):
    init_repo(context)
    context.decision_sha = commit(context, "zmem(DECISION): deep bounded target")
    commit(context, "zmem(LESSON_LEARNT): newer attention")
    context.proposed_message = f"fix: cancel\n\nzmem(CANCEL)[{context.decision_sha[:8]}, 1]"
    _query(context)
    _assert_success(context)
    with database(context) as connection:
        context.persistent_before = (
            connection.execute("SELECT COUNT(*) FROM entries").fetchone()[0],
            connection.execute("SELECT attention_identity FROM anchors").fetchone()[0],
        )


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
    with database(context) as connection:
        after = (
            connection.execute("SELECT COUNT(*) FROM entries").fetchone()[0],
            connection.execute("SELECT attention_identity FROM anchors").fetchone()[0],
        )
    assert after == context.persistent_before


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
