"""
Aeroboard Arrow schemas for telemetry data.

These schemas define the structure of telemetry data sent to the Aeroboard
backend via gRPC Flight protocol.
"""

import pyarrow as pa


# Define strict Arrow schemas for telemetry data

scalar_schema = pa.schema([
    ("timestamp", pa.timestamp('ms')),
    ("run_id", pa.string()),
    ("name", pa.string()),
    ("value", pa.float64()),
    ("step", pa.float64()),  # training step shared by the values of one log() call; nullable
])

image_schema = pa.schema([
    ("timestamp", pa.timestamp('ms')),
    ("run_id", pa.string()),
    ("name", pa.string()),
    ("image_data", pa.binary()),  # Encoded WebP/JPEG bytes
    ("format", pa.string())       # "webp", "jpeg", etc.
])

def _media_schema(data_column: str) -> pa.Schema:
    return pa.schema([
        ("timestamp", pa.timestamp('ms')),
        ("run_id", pa.string()),
        ("name", pa.string()),
        (data_column, pa.binary()),  # Encoded container bytes (wav, mp3, mp4, webm, ...)
        ("format", pa.string()),     # File extension, e.g. "wav" or "mp4"
    ])


audio_schema = _media_schema("audio_data")
video_schema = _media_schema("video_data")

hyperparameter_schema = pa.schema([
    ("timestamp", pa.timestamp('ms')),
    ("run_id", pa.string()),
    ("key", pa.string()),
    ("value", pa.string()),
    ("value_type", pa.string())
])