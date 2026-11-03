from unittest.mock import patch
import importlib.util

import pyarrow as pa
import pytest

pytestmark = pytest.mark.legacy_server

import aeroboard_client
import aeroboard_client.client as aeroboard_client_module
from aeroboard_client.client import AeroboardClient, AeroboardLightningCallback, AeroboardLightningLogger, AeroboardRayTuneLoggerCallback, _coerce_image_bytes, _normalize_hparam_value


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

    def do_put(self, descriptor, schema, options=None):
        writer = DummyWriter()
        self.calls.append((descriptor, schema, writer))
        return writer, object()

    def close(self):
        return None


class DummyScalar:
    def __init__(self, value):
        self._value = value

    def item(self):
        return self._value


class FakeTrial:
    def __init__(self, trial_id='trial-001', experiment_tag='lr=0.01', config=None):
        self.trial_id = trial_id
        self.experiment_tag = experiment_tag
        self.trial_name = f'trainable_{trial_id}'
        self.config = config or {'lr': 0.01, 'optimizer': 'adam'}


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
        assert client._image_buffer == []  # handed to the background sender
        client.flush()

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
        assert client._scalar_buffer == []  # handed to the background sender
        client.flush()

        assert len(client.client.calls) == 1
        descriptor, schema, writer = client.client.calls[0]
        assert descriptor.path == [b'scalars']
        assert schema.names == ['timestamp', 'run_id', 'name', 'value', 'step']
        assert writer.batches[0].num_rows == 2


def test_log_metrics_adds_prefix_and_shared_step():
    with patch('aeroboard_client.client.fl.FlightClient', DummyFlightClient):
        client = AeroboardClient(run_id='demo', batch_size=8)
        client.log_metrics(
            {'loss': DummyScalar(0.3), 'accuracy': 0.8, 'optimizer/lr': 0.01, 'ignored': 'text'},
            step=5,
            prefix='train',
        )
        client.flush()

        descriptor, schema, writer = client.client.calls[0]
        assert descriptor.path == [b'scalars']
        batch = writer.batches[0].to_pydict()
        assert batch['name'] == ['train/loss', 'train/accuracy', 'optimizer/lr']
        assert batch['value'] == [0.3, 0.8, 0.01]
        assert batch['step'] == [5.0, 5.0, 5.0]


def test_log_takes_step_from_argument_or_metrics_dict():
    with patch('aeroboard_client.client.fl.FlightClient', DummyFlightClient):
        client = AeroboardClient(run_id='demo', batch_size=100)
        client.log({'loss': 0.5, 'step': 3})
        client.log({'loss': 0.4}, step=4)
        client.log({'loss': 0.3})
        client.flush()

        batch = client.client.calls[0][2].batches[0].to_pydict()
        assert batch['name'] == ['loss', 'loss', 'loss']
        assert batch['step'] == [3.0, 4.0, None]


def test_lightning_logger_reuses_existing_client():
    if importlib.util.find_spec('lightning') is None and importlib.util.find_spec('pytorch_lightning') is None:
        pytest.skip('Lightning is not installed')

    with patch('aeroboard_client.client.fl.FlightClient', DummyFlightClient):
        client = AeroboardClient(run_id='demo', batch_size=2)
        logger = client.lightning_logger()

        assert isinstance(logger, AeroboardLightningLogger)
        assert logger.client is client


def test_attach_lightning_returns_logger_for_compatibility():
    if importlib.util.find_spec('lightning') is None and importlib.util.find_spec('pytorch_lightning') is None:
        pytest.skip('Lightning is not installed')

    with patch('aeroboard_client.client.fl.FlightClient', DummyFlightClient):
        client = AeroboardClient(run_id='demo', batch_size=2)
        with pytest.deprecated_call():
            logger = client.attach_lightning()

        assert isinstance(logger, AeroboardLightningLogger)
        assert logger.client is client


def test_lightning_logger_logs_numeric_metrics_and_step():
    if importlib.util.find_spec('lightning') is None and importlib.util.find_spec('pytorch_lightning') is None:
        pytest.skip('Lightning is not installed')

    with patch('aeroboard_client.client.fl.FlightClient', DummyFlightClient):
        client = AeroboardClient(run_id='demo', batch_size=8)
        logger = client.lightning_logger()

        logger.log_metrics({'train/loss': 0.25, 'epoch': 3, 'ignore': 'text'}, step=7)
        client.flush()

        descriptor, schema, writer = client.client.calls[0]
        assert descriptor.path == [b'scalars']
        batch = writer.batches[0].to_pydict()
        assert batch['name'] == ['train/loss', 'epoch']
        assert batch['value'] == [0.25, 3.0]
        assert batch['step'] == [7.0, 7.0]


def test_ray_tune_callback_creates_trial_run_and_logs_metrics(monkeypatch):
    with patch('aeroboard_client.client.fl.FlightClient', DummyFlightClient):
        monkeypatch.setattr(aeroboard_client_module, '_RAY_TUNE_AVAILABLE', True)
        callback = AeroboardRayTuneLoggerCallback(
            url='grpc://localhost:50051',
            api_base_url='http://localhost:3000',
            batch_size=8,
            run_prefix='ray-demo',
            metric_prefix='train',
        )
        trial = FakeTrial()

        callback.log_trial_start(trial)
        client = callback._clients[trial.trial_id]
        callback.log_trial_result(4, trial, {
            'loss': 0.4,
            'accuracy': 0.75,
            'training_iteration': 4,
            'pid': 12345,
            'hostname': 'worker-1',
            'ignore': 'text',
        })
        callback.log_trial_end(trial)

        assert client.base_run_id == 'ray-demo-trainable_trial-001'
        assert len(client.client.calls) == 2

        scalar_descriptor, scalar_schema, scalar_writer = client.client.calls[0]
        assert scalar_descriptor.path == [b'scalars']
        scalars = scalar_writer.batches[0].to_pydict()
        assert scalars['name'] == ['train/loss', 'train/accuracy']
        assert scalars['value'] == [0.4, 0.75]
        assert scalars['step'] == [4.0, 4.0]

        hyper_descriptor, hyper_schema, hyper_writer = client.client.calls[1]
        assert hyper_descriptor.path == [b'hyperparameters']
        assert hyper_schema.names == ['timestamp', 'run_id', 'key', 'value', 'value_type']
        hyper_keys = hyper_writer.batches[0].column(2).to_pylist()
        assert 'lr' in hyper_keys
        assert 'optimizer' in hyper_keys
        assert 'ray/trial_id' in hyper_keys
        assert 'ray/experiment_tag' in hyper_keys
        assert 'ray/failed' in hyper_keys


def test_module_level_helpers_use_default_client():
    with patch('aeroboard_client.client.fl.FlightClient', DummyFlightClient):
        client = aeroboard_client.init(run_id='demo', batch_size=2)
        aeroboard_client.log_metrics({'loss': 0.3}, step=1, prefix='train')
        aeroboard_client.log_metrics({'loss': 0.2}, step=2, prefix='train')
        aeroboard_client.flush()

        assert aeroboard_client.get_default_client() is client
        assert len(client.client.calls) == 1
        descriptor, schema, writer = client.client.calls[0]
        assert descriptor.path == [b'scalars']
        assert writer.batches[0].to_pydict()['step'] == [1.0, 2.0]
