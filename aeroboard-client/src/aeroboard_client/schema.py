import pyarrow as pa

# Define strict Arrow schemas for scalars

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
