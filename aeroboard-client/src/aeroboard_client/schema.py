import pyarrow as pa

from aeroboard.schema import *  # noqa: F403

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
    ("image_data", pa.binary()), # Encoded WebP/JPEG bytes
    ("format", pa.string())      # "webp", "jpeg", etc.
])

distribution_schema = pa.schema([
    ("timestamp", pa.timestamp('ms')),
    ("run_id", pa.string()),
    ("name", pa.string()),
    ("values", pa.list_(pa.float64()))
])

hyperparameter_schema = pa.schema([
    ("timestamp", pa.timestamp('ms')),
    ("run_id", pa.string()),
    ("key", pa.string()),
    ("value", pa.string()),
    ("value_type", pa.string())
])
