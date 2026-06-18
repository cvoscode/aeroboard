import os


DEFAULT_EXAMPLE_FLIGHT_URL = "grpc://localhost:50051"
DEFAULT_EXAMPLE_API_BASE_URL = "http://localhost:3000"


def get_example_flight_url() -> str:
    return os.getenv("AEROBOARD_EXAMPLE_FLIGHT_URL", DEFAULT_EXAMPLE_FLIGHT_URL)


def get_example_api_base_url() -> str:
    return os.getenv("AEROBOARD_EXAMPLE_API_BASE_URL", DEFAULT_EXAMPLE_API_BASE_URL)
