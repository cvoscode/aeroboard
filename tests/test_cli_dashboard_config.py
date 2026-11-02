from types import SimpleNamespace

import pytest
from click.testing import CliRunner

from aeroboard_client import cli


@pytest.fixture
def start_server(tmp_path, monkeypatch):
    project_dir = tmp_path / "project"
    project_dir.mkdir()
    runtime_dir = tmp_path / "runtime"
    runtime_dir.mkdir()
    monkeypatch.chdir(project_dir)
    monkeypatch.delenv("AEROBOARD_DASHBOARD_CONFIG", raising=False)
    monkeypatch.setattr(
        cli,
        "_prepare_runtime_assets",
        lambda build_from_source, force_download: (str(runtime_dir / "aeroboard"), str(runtime_dir / "dist")),
    )
    launched = {}

    def run(command, **kwargs):
        launched.update(kwargs)
        return SimpleNamespace(returncode=0)

    monkeypatch.setattr(cli.subprocess, "run", run)

    def start():
        result = CliRunner().invoke(cli.main, ["start"])
        assert result.exit_code == 0, result.output
        return launched

    return start


@pytest.mark.parametrize("unset_value", [None, "", "   "])
def test_start_finds_config_in_callers_directory(start_server, tmp_path, monkeypatch, unset_value):
    project_dir = tmp_path / "project"
    config_path = project_dir / "aeroboard.toml"
    config_path.write_text('[dashboard]\nstarred = ["loss"]\n')
    if unset_value is not None:
        monkeypatch.setenv("AEROBOARD_DASHBOARD_CONFIG", unset_value)

    launched = start_server()

    assert launched["cwd"] == str(tmp_path / "runtime")
    assert launched["env"]["AEROBOARD_DASHBOARD_CONFIG"] == str(config_path)


@pytest.mark.parametrize("absolute", [False, True])
def test_explicit_config_path_overrides_project_default(start_server, tmp_path, monkeypatch, absolute):
    project_dir = tmp_path / "project"
    (project_dir / "aeroboard.toml").write_text("[dashboard]\n")
    config_path = project_dir / "settings" / "dashboard.toml"
    config_path.parent.mkdir()
    config_path.write_text('[dashboard]\nstarred = ["accuracy"]\n')
    configured_path = str(config_path) if absolute else "settings/dashboard.toml"
    monkeypatch.setenv("AEROBOARD_DASHBOARD_CONFIG", configured_path)

    launched = start_server()

    assert launched["env"]["AEROBOARD_DASHBOARD_CONFIG"] == str(config_path)


def test_missing_explicit_config_reaches_server_for_error_reporting(start_server, tmp_path, monkeypatch):
    project_dir = tmp_path / "project"
    (project_dir / "aeroboard.toml").write_text("[dashboard]\n")
    monkeypatch.setenv("AEROBOARD_DASHBOARD_CONFIG", "missing.toml")

    launched = start_server()

    assert launched["env"]["AEROBOARD_DASHBOARD_CONFIG"] == str(project_dir / "missing.toml")


def test_start_without_config_keeps_it_optional(start_server):
    launched = start_server()

    assert "AEROBOARD_DASHBOARD_CONFIG" not in launched["env"]
