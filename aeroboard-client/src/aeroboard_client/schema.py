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
    ("value", pa.float64())
])

image_schema = pa.schema([
    ("timestamp", pa.timestamp('ms')),
    ("run_id", pa.string()),
    ("name", pa.string()),
    ("image_data", pa.binary()),  # Encoded WebP/JPEG bytes
    ("format", pa.string())       # "webp", "jpeg", etc.
])

hyperparameter_schema = pa.schema([
    ("timestamp", pa.timestamp('ms')),
    ("run_id", pa.string()),
    ("key", pa.string()),
    ("value", pa.string()),
    ("value_type", pa.string())
])