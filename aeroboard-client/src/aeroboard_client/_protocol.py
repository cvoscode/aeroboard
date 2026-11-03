"""Canonical batch identities shared with the Rust ingestion service."""
import hashlib
import math
import struct

import pyarrow as pa

ENVELOPE_KEY = b"aeroboard.envelope"


def batch_checksum(batch):
    digest = hashlib.sha256()
    digest.update(struct.pack("<Q", batch.num_rows))
    for field, column in zip(batch.schema, batch.columns):
        name = field.name.encode("utf-8")
        digest.update(struct.pack("<I",len(name)))
        digest.update(name)
        if pa.types.is_string(field.type):
            tag = b"s"
        elif pa.types.is_binary(field.type):
            tag = b"b"
        elif pa.types.is_float64(field.type):
            tag = b"f"
        elif pa.types.is_timestamp(field.type):
            tag = b"t"
            column = column.cast(pa.int64())
        else:
            raise ValueError(f"Unsupported protocol type: {field.type}")
        digest.update(tag)
        for value in column.to_pylist():
            if value is None:
                digest.update(b"\0")
                continue
            digest.update(b"\1")
            if tag in (b"s",b"b"):
                encoded = value.encode("utf-8") if tag == b"s" else value
                digest.update(struct.pack("<Q",len(encoded)))
                digest.update(encoded)
            elif tag == b"f":
                digest.update(struct.pack("<Q",0x7ff8000000000000) if math.isnan(value) else struct.pack("<d",value))
            else:
                digest.update(struct.pack("<q",value))
    return digest.hexdigest()


def media_metadata(batch, rows):
    return pa.RecordBatch.from_arrays([*batch.columns,
        pa.array([row["asset_id"] for row in rows],type=pa.string()),
        pa.array([row.get("step") for row in rows],type=pa.float64()),
        pa.array([row.get("segment") for row in rows],type=pa.string())],
        schema=pa.schema([*batch.schema,pa.field("asset_id",pa.string()),pa.field("step",pa.float64()),pa.field("segment",pa.string())]))
