from unittest.mock import patch
import importlib.util

import pyarrow as pa
import pytest

import aeroboard_client
from aeroboard_client.client import AeroboardClient, AeroboardLightningCallback, _coerce_image_bytes, _normalize_hparam_value


class DummyWriter:
    def __init__(self):
        self.batches = []
        self.done = False
        self.closed = False

    def write_batch(self, batch):
        self.batches.append(batch)

    def done_writing(self):
        self.done = True

    def close(self):
        self.closed = True


class DummyFlightClient:
    def __init__(self, url):
        self.url = url
        self.calls = []

    def do_put(self, descriptor, schema):
        writer = DummyWriter()
        self.calls.append((descriptor, schema, writer))
        return writer, object()

    def close(self):
        return None


def test_normalize_hparam_value_supports_common_types():
    assert _normalize_hparam_value(3) == ("3", "int")
    assert _normalize_hparam_value(0.5) == ("0.5", "float")
    assert _normalize_hparam_value(True) == ("true", "bool")
    assert _normalize_hparam_value("adam") == ("adam", "string")
    assert _normalize_hparam_value([1, 2]) == ("1, 2", "list")


def test_normalize_hparam_value_ignores_none():
    assert _normalize_hparam_value(None) is None


def test_coerce_image_bytes_accepts_raw_bytes():
    payload, fmt = _coerce_image_bytes(b"abc", format="png")
    assert payload == b"abc"
    assert fmt == "png"


def test_coerce_image_bytes_compresses_large_images_with_pillow():
    pytest = __import__('pytest')
    Image = pytest.importorskip('PIL.Image')
    image = Image.new('RGB', (512, 512), color=(120, 30, 220))
    payload, fmt = _coerce_image_bytes(image, format='png', max_bytes=2048)
    assert fmt in {'png', 'webp'}
    assert len(payload) > 0


def test_hyperparameter_flush_uses_hparam_descriptor():
    with patch('aeroboard_client.client.fl.FlightClient', DummyFlightClient):
        client = AeroboardClient(run_id='demo', batch_size=4)
        client.log_hyperparameters({'lr': 0.1, 'optimizer': 'adam'})
        client.flush()

        assert len(client.client.calls) == 1
        descriptor, schema, writer = client.client.calls[0]
        assert descriptor.descriptor_type.name == 'PATH'
        assert descriptor.path == [b'hyperparameters']
        assert schema.names == ['timestamp', 'run_id', 'key', 'value', 'value_type']
        batch = writer.batches[0]
        assert isinstance(batch, pa.RecordBatch)
        assert batch.num_rows == 2
        assert client._hyperparameter_buffer == []


def test_image_batch_flush_triggers_at_quarter_batch_size():
    with patch('aeroboard_client.client.fl.FlightClient', DummyFlightClient):
        client = AeroboardClient(run_id='demo', batch_size=8)
        client.log_image('samples/frame', b'raw-image', format='png')
        client.log_image('samples/frame', b'raw-image-2', format='png')

        assert len(client.client.calls) == 1
        descriptor, schema, writer = client.client.calls[0]
        assert descriptor.path == [b'images']
        assert schema.names == ['timestamp', 'run_id', 'name', 'image_data', 'format']
        assert writer.batches[0].num_rows == 2
        assert client._image_buffer == []


def test_scalar_batch_flushes_when_batch_limit_reached():
    with patch('aeroboard_client.client.fl.FlightClient', DummyFlightClient):
        client = AeroboardClient(run_id='demo', batch_size=2)
        client.log({'train/loss': 0.3})
        assert len(client.client.calls) == 0
        client.log({'train/loss': 0.2})

        assert len(client.client.calls) == 1
        descriptor, schema, writer = client.client.calls[0]
        assert descriptor.path == [b'scalars']
        assert schema.names == ['timestamp', 'run_id', 'name', 'value']
        assert writer.batches[0].num_rows == 2
        assert client._scalar_buffer == []


def test_attach_lightning_reuses_existing_client():
    if importlib.util.find_spec('lightning') is None and importlib.util.find_spec('pytorch_lightning') is None:
        pytest.skip('Lightning is not installed')

    with patch('aeroboard_client.client.fl.FlightClient', DummyFlightClient):
        client = AeroboardClient(run_id='demo', batch_size=2)
        callback = client.attach_lightning()

        assert isinstance(callback, AeroboardLightningCallback)
        assert callback.client is client


def test_module_level_helpers_use_default_client():
    with patch('aeroboard_client.client.fl.FlightClient', DummyFlightClient):
        client = aeroboard_client.init(run_id='demo', batch_size=2)
        aeroboard_client.log({'train/loss': 0.3})
        aeroboard_client.log({'train/loss': 0.2})

        assert aeroboard_client.get_default_client() is client
        descriptor, schema, writer = client.client.calls[0]
        assert descriptor.path == [b'scalars']
        assert schema.names == ['timestamp', 'run_id', 'name', 'value']
        assert writer.batches[0].num_rows == 2
