"""Compatibility naming for servers predating lifecycle registration."""
import os
import re
import time
import uuid
from typing import Optional
def _normalize_run_id(run_id: Optional[str]) -> str:
    """Normalize a run ID by stripping whitespace."""
    cleaned = (run_id or '').strip()
    return cleaned or f"run-{int(time.time())}"


def _default_api_base_url(flight_url: Optional[str] = None) -> str:
    if flight_url is None:
        return "http://localhost:3000"

    if flight_url.startswith("grpc://"):
        return flight_url.replace("grpc://", "http://").replace(":50051", ":3000")
    if flight_url.startswith("grpcs://"):
        return flight_url.replace("grpcs://", "https://").replace(":50051", ":3000")
    return "http://localhost:3000"


def _fetch_existing_run_ids(base: str, api_base_url: Optional[str] = None, *, session=None, verify=True) -> set[str]:
    """Fetch run IDs matching ``base`` (or its versions) from the server's run list.

    Only used when the server's ``/api/runs/resolve`` endpoint is unavailable. The
    client never talks to the storage backend directly.
    """
    import requests

    normalized_base = _normalize_run_id(base)
    api_base_url = (api_base_url or "http://localhost:3000").rstrip("/")
    try:
        resp = (session or requests).get(f"{api_base_url}/api/runs", timeout=2, verify=verify)
        if resp.status_code != 200:
            return set()
        runs = resp.json().get("runs", [])
    except Exception:
        return set()
    return {
        run_id for run_id in runs
        if run_id == normalized_base or run_id.startswith(f"{normalized_base}-v")
    }


def resolve_run_id(base: str, existing: set[str], include_base: bool = True) -> str:
    """
    Resolve a run ID with versioning.

    Args:
        base: Base run name.
        existing: Set of existing run IDs.
        include_base: Whether to include the base run if version 1 doesn't exist.

    Returns:
        The resolved run ID with version suffix if needed.
    """
    normalized = _normalize_run_id(base)
    base_without_suffix, framework_suffix = _split_framework_suffix(normalized)

    existing_versions = {
        version
        for candidate in existing
        if (version := _extract_version(candidate, normalized)) is not None
    }

    if include_base and 1 not in existing_versions:
        return normalized

    next_version = max(existing_versions, default=1) + 1
    return f"{base_without_suffix}-v{next_version}{framework_suffix}"


def _split_framework_suffix(candidate: str) -> tuple[str, str]:
    """
    Split a run ID into base name and framework suffix.

    Args:
        candidate: The run ID to split.

    Returns:
        Tuple of (base_name, framework_suffix).
    """
    # A Lightning suffix is "-pl" plus a dotted version ("exp-pl2.5.1"); names that
    # merely contain "-pl" ("sample-placement") have none.
    match = re.fullmatch(r"(.+?)(-pl\d+(?:\.\d+)*)?", candidate)
    if match is None:
        return candidate, ""
    return match.group(1), match.group(2) or ""


def _extract_version(candidate: str, base: str) -> Optional[int]:
    """
    Extract version number from a candidate run ID relative to base.

    Args:
        candidate: The candidate run ID.
        base: The base run name.

    Returns:
        Version number or None if no match.
    """
    candidate_base, candidate_suffix = _split_framework_suffix(candidate)
    base_without_suffix, base_suffix = _split_framework_suffix(base)

    if candidate_suffix != base_suffix:
        return None

    if candidate_base == base_without_suffix:
        return 1

    match = re.fullmatch(rf"{re.escape(base_without_suffix)}-v(\d+)", candidate_base)
    if match is None:
        return None
    return int(match.group(1))

