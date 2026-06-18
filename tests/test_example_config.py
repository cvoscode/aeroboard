import importlib.util
import sys
from pathlib import Path


def load_example_config():
    module_path = Path(__file__).resolve().parents[1] / "examples" / "_config.py"
    spec = importlib.util.spec_from_file_location("aeroboard_example_config", module_path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def test_example_flight_url_defaults(monkeypatch):
    monkeypatch.delenv("AEROBOARD_EXAMPLE_FLIGHT_URL", raising=False)

    config = load_example_config()

    assert config.get_example_flight_url() == "grpc://localhost:50051"


def test_example_flight_url_honors_env_override(monkeypatch):
    monkeypatch.setenv("AEROBOARD_EXAMPLE_FLIGHT_URL", "grpc://localhost:51010")

    config = load_example_config()

    assert config.get_example_flight_url() == "grpc://localhost:51010"


def test_example_api_base_url_honors_env_override(monkeypatch):
    monkeypatch.setenv("AEROBOARD_EXAMPLE_API_BASE_URL", "http://localhost:3010")

    config = load_example_config()

    assert config.get_example_api_base_url() == "http://localhost:3010"
