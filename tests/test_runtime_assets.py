from pathlib import Path

import pytest

from aeroboard_client import runtime


def test_release_urls_follow_package_version(monkeypatch):
    monkeypatch.delenv("AEROBOARD_RELEASE_BASE_URL", raising=False)
    monkeypatch.delenv("AEROBOARD_GITHUB_REPOSITORY", raising=False)

    assert runtime.release_tag() == "v0.1.0"
    assert runtime.server_asset_name(runtime.PlatformSpec("linux-x86_64", "tar.gz", "aeroboard")) == "aeroboard-server-v0.1.0-linux-x86_64.tar.gz"
    assert runtime.web_asset_name() == "aeroboard-web-v0.1.0.tar.gz"
    assert runtime.release_base_url() == "https://github.com/cvoscode/aeroboard/releases/download/v0.1.0"


def test_detect_platform_normalizes_common_aliases(monkeypatch):
    monkeypatch.setattr(runtime.sys, "platform", "linux")
    monkeypatch.setattr(runtime.platform, "machine", lambda: "amd64")

    spec = runtime.detect_platform()

    assert spec.slug == "linux-x86_64"
    assert spec.archive_extension == "tar.gz"
    assert spec.binary_name == "aeroboard"


def test_ensure_server_binary_honors_env_override(tmp_path, monkeypatch):
    binary_path = tmp_path / "aeroboard"
    binary_path.write_text("binary")
    monkeypatch.setenv("AEROBOARD_SERVER_BINARY", str(binary_path))

    resolved = runtime.ensure_server_binary()

    assert resolved == binary_path.resolve()


def test_ensure_web_dist_honors_env_override(tmp_path, monkeypatch):
    dist_dir = tmp_path / "dist"
    dist_dir.mkdir()
    (dist_dir / "index.html").write_text("<html></html>")
    monkeypatch.setenv("AEROBOARD_WEB_DIST_DIR", str(dist_dir))

    resolved = runtime.ensure_web_dist()

    assert resolved == dist_dir.resolve()


def test_ensure_web_dist_rejects_missing_index(tmp_path, monkeypatch):
    dist_dir = tmp_path / "dist"
    dist_dir.mkdir()
    monkeypatch.setenv("AEROBOARD_WEB_DIST_DIR", str(dist_dir))

    with pytest.raises(runtime.RuntimeAssetError):
        runtime.ensure_web_dist()