from __future__ import annotations

import hashlib
import os
import platform
import shutil
import stat
import subprocess
import sys
import tarfile
import urllib.error
import urllib.request
import zipfile
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from aeroboard_client._version import __version__


DEFAULT_GITHUB_REPOSITORY = "cvoscode/aeroboard"
GREPTIME_CONTAINER_NAME = "aeroboard-greptimedb"
GREPTIME_VOLUME_NAME = "aeroboard-greptimedb-data"
Reporter = Callable[[str], None]


class RuntimeAssetError(RuntimeError):
    pass


@dataclass(frozen=True)
class PlatformSpec:
    slug: str
    archive_extension: str
    binary_name: str


def package_version() -> str:
    return __version__


def release_tag() -> str:
    return f"v{package_version()}"


def github_repository() -> str:
    return os.environ.get("AEROBOARD_GITHUB_REPOSITORY", DEFAULT_GITHUB_REPOSITORY)


def release_base_url() -> str:
    override = os.environ.get("AEROBOARD_RELEASE_BASE_URL")
    if override:
        return override.rstrip("/")
    return f"https://github.com/{github_repository()}/releases/download/{release_tag()}"


def runtime_cache_dir() -> Path:
    cache_home = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache"))
    return cache_home / "aeroboard" / release_tag()


def detect_platform() -> PlatformSpec:
    machine = platform.machine().lower()
    if machine in {"x86_64", "amd64"}:
        arch = "x86_64"
    elif machine in {"aarch64", "arm64"}:
        arch = "aarch64"
    else:
        raise RuntimeAssetError(f"Unsupported CPU architecture: {machine}")

    if sys.platform.startswith("linux"):
        os_name = "linux"
        archive_extension = "tar.gz"
        binary_name = "aeroboard"
    elif sys.platform == "darwin":
        os_name = "macos"
        archive_extension = "tar.gz"
        binary_name = "aeroboard"
    elif sys.platform in {"win32", "cygwin"}:
        os_name = "windows"
        archive_extension = "zip"
        binary_name = "aeroboard.exe"
    else:
        raise RuntimeAssetError(f"Unsupported operating system: {sys.platform}")

    return PlatformSpec(
        slug=f"{os_name}-{arch}",
        archive_extension=archive_extension,
        binary_name=binary_name,
    )


def server_asset_name(spec: PlatformSpec | None = None) -> str:
    resolved_spec = spec or detect_platform()
    return f"aeroboard-server-{release_tag()}-{resolved_spec.slug}.{resolved_spec.archive_extension}"


def web_asset_name() -> str:
    return f"aeroboard-web-{release_tag()}.tar.gz"


def _asset_url(asset_name: str) -> str:
    return f"{release_base_url()}/{asset_name}"


def find_project_root() -> Path | None:
    current = Path(__file__).resolve()
    for parent in current.parents:
        if (parent / "aeroboard-server" / "Cargo.toml").exists() and (parent / "aeroboard-web" / "package.json").exists():
            return parent
    return None


def _ensure_directory(path: Path) -> None:
    path.mkdir(parents=True, exist_ok=True)


def _default_reporter(_: str) -> None:
    return None


def _run(command: list[str], *, cwd: Path | None = None, reporter: Reporter | None = None) -> None:
    active_reporter = reporter or _default_reporter
    active_reporter(f"$ {' '.join(command)}")
    subprocess.run(command, cwd=cwd, check=True)


def _download_asset(asset_name: str, destination: Path) -> Path:
    _ensure_directory(destination.parent)
    request = urllib.request.Request(
        _asset_url(asset_name),
        headers={"User-Agent": f"aeroboard/{package_version()}"},
    )
    temporary=destination.with_name(destination.name+".partial")
    try:
        manifest_request=urllib.request.Request(_asset_url("SHA256SUMS"),headers={"User-Agent":f"aeroboard/{package_version()}"})
        with urllib.request.urlopen(manifest_request,timeout=30) as response:
            manifest=response.read().decode("utf-8")
        checksums={parts[1].lstrip("*"):parts[0] for line in manifest.splitlines() if len(parts:=line.split())==2}
        expected=checksums.get(asset_name)
        if expected is None or len(expected)!=64:
            raise RuntimeAssetError(f"Release checksum missing for '{asset_name}'")
        digest=hashlib.sha256()
        with urllib.request.urlopen(request,timeout=60) as response,temporary.open("wb") as handle:
            while chunk:=response.read(1024*1024):digest.update(chunk);handle.write(chunk)
        if digest.hexdigest()!=expected.lower():raise RuntimeAssetError(f"Release checksum mismatch for '{asset_name}'")
        temporary.replace(destination)
    except (urllib.error.URLError,OSError,UnicodeError) as exc:
        raise RuntimeAssetError(f"Failed to download verified release artifact '{asset_name}': {exc}") from exc
    finally:
        temporary.unlink(missing_ok=True)

    return destination


def _safe_extract_tar(archive_path: Path, destination: Path) -> None:
    with tarfile.open(archive_path, "r:*") as archive:
        for member in archive.getmembers():
            target = destination / member.name
            if not target.resolve().is_relative_to(destination.resolve()):
                raise RuntimeAssetError(f"Unsafe path in archive '{archive_path}': {member.name}")
        archive.extractall(destination)


def _extract_archive(archive_path: Path, destination: Path) -> None:
    if destination.exists():
        shutil.rmtree(destination)
    _ensure_directory(destination)

    if archive_path.suffix == ".zip":
        with zipfile.ZipFile(archive_path) as archive:
            archive.extractall(destination)
        return

    _safe_extract_tar(archive_path, destination)


def _mark_executable(path: Path) -> None:
    current_mode = path.stat().st_mode
    path.chmod(current_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def _resolve_existing_child(root: Path, candidates: list[str]) -> Path | None:
    for candidate in candidates:
        candidate_path = root / candidate
        if candidate_path.exists():
            return candidate_path
    for candidate in candidates:
        matches = list(root.rglob(candidate))
        if matches:
            return matches[0]
    return None


def build_server_from_source(project_root: Path, reporter: Reporter | None = None) -> Path:
    server_dir = project_root / "aeroboard-server"
    _run(["cargo", "build", "--release"], cwd=server_dir, reporter=reporter)
    binary_path = server_dir / "target" / "release" / detect_platform().binary_name
    if not binary_path.exists():
        raise RuntimeAssetError(f"Expected built server binary at {binary_path}")
    return binary_path


def build_web_from_source(project_root: Path, reporter: Reporter | None = None) -> Path:
    web_dir = project_root / "aeroboard-web"
    _run(["npm", "install"], cwd=web_dir, reporter=reporter)
    _run(["npm", "run", "build"], cwd=web_dir, reporter=reporter)
    dist_dir = web_dir / "dist"
    if not (dist_dir / "index.html").exists():
        raise RuntimeAssetError(f"Expected built web assets at {dist_dir}")
    return dist_dir


def _download_server_binary(spec: PlatformSpec, reporter: Reporter | None = None) -> Path:
    active_reporter = reporter or _default_reporter
    cache_root = runtime_cache_dir() / "server" / spec.slug
    binary_path = cache_root / spec.binary_name
    if binary_path.exists():
        return binary_path

    _ensure_directory(cache_root)
    archive_path = cache_root / server_asset_name(spec)
    active_reporter(f"Downloading Aeroboard server binary: {server_asset_name(spec)}")
    _download_asset(server_asset_name(spec), archive_path)
    extract_root = cache_root / "extract"
    _extract_archive(archive_path, extract_root)
    extracted_binary = _resolve_existing_child(extract_root, [spec.binary_name, f"bin/{spec.binary_name}"])
    if extracted_binary is None:
        raise RuntimeAssetError(f"Downloaded server archive did not contain {spec.binary_name}")
    shutil.copy2(extracted_binary, binary_path)
    _mark_executable(binary_path)
    return binary_path


def _download_web_dist(reporter: Reporter | None = None) -> Path:
    active_reporter = reporter or _default_reporter
    cache_root = runtime_cache_dir() / "web"
    dist_dir = cache_root / "dist"
    if (dist_dir / "index.html").exists():
        return dist_dir

    _ensure_directory(cache_root)
    archive_path = cache_root / web_asset_name()
    active_reporter(f"Downloading Aeroboard web bundle: {web_asset_name()}")
    _download_asset(web_asset_name(), archive_path)
    extract_root = cache_root / "extract"
    _extract_archive(archive_path, extract_root)
    extracted_dist = _resolve_existing_child(extract_root, ["dist", "web", "index.html"])
    if extracted_dist is None:
        raise RuntimeAssetError("Downloaded web archive did not contain a dist directory")
    if extracted_dist.is_file():
        dist_source = extracted_dist.parent
    else:
        dist_source = extracted_dist
    if dist_dir.exists():
        shutil.rmtree(dist_dir)
    shutil.copytree(dist_source, dist_dir)
    return dist_dir


def ensure_server_binary(
    *,
    prefer_local_build: bool = False,
    force_download: bool = False,
    reporter: Reporter | None = None,
) -> Path:
    override = os.environ.get("AEROBOARD_SERVER_BINARY")
    if override:
        resolved = Path(override).expanduser().resolve()
        if not resolved.exists():
            raise RuntimeAssetError(f"AEROBOARD_SERVER_BINARY does not exist: {resolved}")
        return resolved

    spec = detect_platform()
    project_root = find_project_root()
    local_binary = None
    if project_root is not None:
        local_binary = project_root / "aeroboard-server" / "target" / "release" / spec.binary_name

    if prefer_local_build and project_root is not None:
        return build_server_from_source(project_root, reporter=reporter)

    if not force_download and local_binary is not None and local_binary.exists():
        return local_binary

    try:
        return _download_server_binary(spec, reporter=reporter)
    except RuntimeAssetError:
        if project_root is not None:
            return build_server_from_source(project_root, reporter=reporter)
        raise


def ensure_web_dist(
    *,
    prefer_local_build: bool = False,
    force_download: bool = False,
    reporter: Reporter | None = None,
) -> Path:
    override = os.environ.get("AEROBOARD_WEB_DIST_DIR")
    if override:
        resolved = Path(override).expanduser().resolve()
        if not (resolved / "index.html").exists():
            raise RuntimeAssetError(f"AEROBOARD_WEB_DIST_DIR does not contain index.html: {resolved}")
        return resolved

    project_root = find_project_root()
    local_dist = None
    if project_root is not None:
        local_dist = project_root / "aeroboard-web" / "dist"

    if prefer_local_build and project_root is not None:
        return build_web_from_source(project_root, reporter=reporter)

    if not force_download and local_dist is not None and (local_dist / "index.html").exists():
        return local_dist

    try:
        return _download_web_dist(reporter=reporter)
    except RuntimeAssetError:
        if project_root is not None:
            return build_web_from_source(project_root, reporter=reporter)
        raise


def start_greptime_container(image: str, reporter: Reporter | None = None) -> None:
    active_reporter = reporter or _default_reporter
    inspect = subprocess.run(
        ["docker", "ps", "-a", "--filter", f"name=^{GREPTIME_CONTAINER_NAME}$", "--format", "{{.Names}}"],
        capture_output=True,
        text=True,
        check=True,
    )
    exists = GREPTIME_CONTAINER_NAME in inspect.stdout.splitlines()
    if exists:
        active_reporter(f"Starting existing container {GREPTIME_CONTAINER_NAME}")
        subprocess.run(["docker", "start", GREPTIME_CONTAINER_NAME], check=True)
        return

    active_reporter(f"Creating container {GREPTIME_CONTAINER_NAME}")
    subprocess.run(
        [
            "docker",
            "run",
            "-d",
            "--name",
            GREPTIME_CONTAINER_NAME,
            "-p",
            "4000:4000",
            "-p",
            "4001:4001",
            "-p",
            "4002:4002",
            "-p",
            "4003:4003",
            "-v",
            f"{GREPTIME_VOLUME_NAME}:/tmp/greptimedb",
            image,
            "standalone",
            "start",
            "--http-addr",
            "0.0.0.0:4000",
            "--rpc-addr",
            "0.0.0.0:4001",
        ],
        check=True,
    )


def stop_greptime_container(reporter: Reporter | None = None) -> bool:
    active_reporter = reporter or _default_reporter
    inspect = subprocess.run(
        ["docker", "ps", "-a", "--filter", f"name=^{GREPTIME_CONTAINER_NAME}$", "--format", "{{.Names}}"],
        capture_output=True,
        text=True,
        check=True,
    )
    exists = GREPTIME_CONTAINER_NAME in inspect.stdout.splitlines()
    if not exists:
        active_reporter(f"Container {GREPTIME_CONTAINER_NAME} is not running")
        return False
    subprocess.run(["docker", "stop", GREPTIME_CONTAINER_NAME], check=True)
    return True