from pathlib import Path
import sys
import pytest


CLIENT_SRC = Path(__file__).resolve().parents[1] / "aeroboard-client" / "src"
if str(CLIENT_SRC) not in sys.path:
    sys.path.insert(0, str(CLIENT_SRC))


@pytest.fixture(autouse=True)
def legacy_server_capabilities(request,monkeypatch):
    """Legacy fixtures have no workspace endpoint; never probe a local service."""
    if request.node.get_closest_marker("legacy_server"):
        from aeroboard_client._tracking import RunTracker, TrackingError
        def unsupported(self,method,path,payload=None):
            error=TrackingError("HTTP 404: legacy fixture has no workspace API")
            error.status=404
            raise error
        monkeypatch.setattr(RunTracker,"request",unsupported)
