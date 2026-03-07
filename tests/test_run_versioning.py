from aeroboard.client import resolve_run_id, _normalize_run_id


def test_resolve_run_id_keeps_base_when_unused():
    assert resolve_run_id("experiment", set()) == "experiment"


def test_resolve_run_id_increments_highest_existing_version():
    existing = {"experiment", "experiment-v2", "experiment-v4"}
    assert resolve_run_id("experiment", existing) == "experiment-v5"


def test_resolve_run_id_increments_lightning_runs_without_reusing_v1():
    existing = {"experiment-pl2.5.1", "experiment-v2-pl2.5.1"}
    assert resolve_run_id("experiment-pl2.5.1", existing) == "experiment-v3-pl2.5.1"


def test_resolve_run_id_ignores_other_framework_suffixes():
    existing = {"experiment-pl2.4.0", "experiment-v2-pl2.4.0"}
    assert resolve_run_id("experiment-pl2.5.1", existing) == "experiment-pl2.5.1"


def test_normalize_run_id_replaces_missing_default():
    resolved = _normalize_run_id("   ")
    assert resolved.startswith("run-")
    assert resolved != "default_run"