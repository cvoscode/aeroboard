class AeroboardClientError(Exception):
    """Base exception for Aeroboard client errors."""

    pass


class FlightConnectionError(AeroboardClientError):
    """Raised when unable to connect to the Flight server."""

    pass

