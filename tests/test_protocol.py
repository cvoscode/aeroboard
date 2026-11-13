import json
import subprocess
import sys
from pathlib import Path

import pyarrow as pa
import pytest
from aeroboard_client._protocol import batch_checksum, legacy_batch_checksum

VECTORS=json.loads((Path(__file__).parents[1]/'protocol/checksum-vectors.json').read_text())

@pytest.mark.parametrize('vector',VECTORS,ids=lambda v:v['name'])
def test_shared_checksums_and_slices(vector):
    arrays=[]
    for column in vector['columns']:
        tag=column['type'];values=column['values']
        if tag=='f':values=[float(v) if v is not None else None for v in values]
        if tag=='b':values=[bytes.fromhex(v) if v is not None else None for v in values]
        kind={'s':pa.string(),'b':pa.binary(),'f':pa.float64(),'t':pa.timestamp('ms')}[tag]
        # Slicing introduces offsets into buffers and validity bitmaps.
        padded=pa.array([None]+values+[None],type=kind)
        arrays.append(padded.slice(1,len(values)))
    batch=pa.RecordBatch.from_arrays(arrays,names=[c['name'] for c in vector['columns']])
    assert legacy_batch_checksum(batch)==vector['v1']
    assert batch_checksum(batch)==vector['v2']

def test_import_does_not_load_optional_frameworks():
    subprocess.run([sys.executable,'-c',"import sys; import aeroboard_client; assert not any(name in sys.modules for name in ('lightning','pytorch_lightning','torch','ray'))"],check=True)
