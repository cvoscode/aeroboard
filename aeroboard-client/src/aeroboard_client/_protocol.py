"""Canonical batch identities shared with the Rust ingestion service."""
import hashlib
import math
import struct

import pyarrow as pa

ENVELOPE_KEY = b"aeroboard.envelope"


def legacy_batch_checksum(batch):
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


def batch_checksum(batch):
    """v2: hash canonical column buffers, including validity, without Python rows."""
    import pyarrow.compute as pc
    digest=hashlib.sha256()
    digest.update(b"aeroboard-columnar-v2\0")
    digest.update(struct.pack("<Q",batch.num_rows))
    for field,column in zip(batch.schema,batch.columns):
        name=field.name.encode();digest.update(struct.pack("<I",len(name)));digest.update(name)
        if pa.types.is_string(field.type):tag=b"s"
        elif pa.types.is_binary(field.type):tag=b"b"
        elif pa.types.is_float64(field.type):tag=b"f"
        elif pa.types.is_timestamp(field.type):tag=b"t"
        else:raise ValueError(f"Unsupported protocol type: {field.type}")
        digest.update(tag)
        validity=pc.cast(pc.is_valid(column),pa.uint8())
        if len(column):digest.update(memoryview(validity.buffers()[1])[:len(column)])
        if tag in (b"s",b"b"):
            column=pc.fill_null(column,"" if tag==b"s" else b"")
            offsets=pa.Array.from_buffers(pa.int32(),len(column)+1,[None,column.buffers()[1]],offset=column.offset)
            base=offsets[0].as_py();end=offsets[-1].as_py()
            normalized=pc.subtract(offsets,pa.scalar(base,type=pa.int32()))
            digest.update(memoryview(normalized.buffers()[1])[:(len(column)+1)*4])
            if end>base:digest.update(memoryview(column.buffers()[2])[base:end])
        else:
            if tag==b"t":column=column.cast(pa.int64())
            column=pc.fill_null(column,0)
            if tag==b"f":column=pc.if_else(pc.is_nan(column),pa.scalar(float("nan")),column)
            if len(column):digest.update(memoryview(column.buffers()[1])[column.offset*8:(column.offset+len(column))*8])
    return digest.hexdigest()


def media_metadata(batch, rows):
    return pa.RecordBatch.from_arrays([*batch.columns,
        pa.array([row["asset_id"] for row in rows],type=pa.string()),
        pa.array([row.get("step") for row in rows],type=pa.float64()),
        pa.array([row.get("segment") for row in rows],type=pa.string())],
        schema=pa.schema([*batch.schema,pa.field("asset_id",pa.string()),pa.field("step",pa.float64()),pa.field("segment",pa.string())]))


class DeferredBatch:
    def __init__(self,factory,num_rows,nbytes,schema):
        self.factory,self.num_rows,self.nbytes,self.schema=factory,num_rows,nbytes,schema

def prepare_batch(batch):
    import json
    if isinstance(batch,DeferredBatch):batch=batch.factory()
    metadata=dict(batch.schema.metadata or {})
    if ENVELOPE_KEY in metadata:
        envelope=json.loads(metadata[ENVELOPE_KEY])
        if not envelope.get("checksum"):
            envelope["checksum"]=(batch_checksum if envelope.get("version",1)==2 else legacy_batch_checksum)(batch)
            metadata[ENVELOPE_KEY]=json.dumps(envelope).encode()
            batch=batch.replace_schema_metadata(metadata)
    return batch
