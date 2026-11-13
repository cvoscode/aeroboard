from unittest.mock import Mock, patch
import pytest

pytestmark = pytest.mark.legacy_server

from aeroboard_client.client import AeroboardClient, resolve_run_id, _fetch_existing_run_ids, _normalize_run_id


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


def test_resolve_run_id_only_treats_versioned_pl_as_framework_suffix():
    # "-pl" inside an ordinary name must not be split off as a Lightning suffix.
    assert resolve_run_id("sample-placement", {"sample-placement"}) == "sample-placement-v2"
    assert resolve_run_id("demo-plot", {"demo-plot", "demo-plot-v2"}) == "demo-plot-v3"


def test_normalize_run_id_replaces_missing_default():
    resolved = _normalize_run_id("   ")
    assert resolved.startswith("run-")
    assert resolved != "default_run"


def test_fetch_existing_run_ids_uses_server_fallback():
    runs_response = Mock(status_code=200)
    runs_response.json.return_value = {"runs": ["experiment", "experiment-v2", "other-run"]}

    with patch("requests.get", return_value=runs_response) as get:
        assert _fetch_existing_run_ids("experiment") == {"experiment", "experiment-v2"}
    # Only the Aeroboard server is asked, never the storage backend directly.
    assert [call.args[0] for call in get.call_args_list] == ["http://localhost:3000/api/runs"]


def test_client_prefers_server_side_run_resolution():
    resolve_response = Mock(status_code=200)
    resolve_response.json.return_value = {"run_id": "experiment-v3"}

    with patch("aeroboard_client.client.fl.FlightClient"):
        with patch("requests.Session.post", return_value=resolve_response):
            client = AeroboardClient(run_id="experiment")
            assert client.run_id == "experiment-v3"


def test_client_falls_back_to_live_run_listing_when_resolve_endpoint_unavailable():
    runs_response = Mock(status_code=200)
    runs_response.json.return_value = {"runs": ["experiment", "experiment-v2"]}

    with patch("aeroboard_client.client.fl.FlightClient"):
        with patch("requests.Session.post", side_effect=RuntimeError("unavailable")):
            with patch("requests.Session.get", return_value=runs_response):
                client = AeroboardClient(run_id="experiment")
                assert client.run_id == "experiment-v3"
