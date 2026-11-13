import json
from unittest.mock import MagicMock, patch

import pytest

pytestmark = pytest.mark.legacy_server

import aeroboard_client
from aeroboard_client.client import (
    DASHBOARD_CONFIG_KEY,
    DASHBOARD_CONFIG_VALUE_TYPE,
    AeroboardClient,
    _normalize_dashboard_config,
)

from test_logging_payloads import DummyFlightClient


@pytest.fixture
def client():
    with patch('aeroboard_client.client.fl.FlightClient', DummyFlightClient), \
            patch.object(AeroboardClient, '_resolve_run_id', lambda self, base: base):
        yield AeroboardClient(run_id='dash-demo')


def _sent_dashboard_rows(client):
    client.flush()
    rows = []
    for descriptor, _schema, writer in client.client.calls:
        if descriptor.path != [b'hyperparameters']:
            continue
        for batch in writer.batches:
            data = batch.to_pydict()
            for key, value, value_type, run_id in zip(data['key'], data['value'], data['value_type'], data['run_id']):
                if key == DASHBOARD_CONFIG_KEY:
                    rows.append((run_id, json.loads(value), value_type))
    return rows


def test_configure_dashboard_sends_reserved_hyperparameter_immediately(client):
    client.configure_dashboard(starred=['loss', ' '], targets={'val/accuracy': 0.95, 'loss': 1}, order=['loss'])

    rows = _sent_dashboard_rows(client)
    assert rows == [(
        'dash-demo',
        {'starred': ['loss'], 'targets': {'val/accuracy': 0.95, 'loss': 1.0}, 'order': ['loss']},
        DASHBOARD_CONFIG_VALUE_TYPE,
    )]


def test_configure_dashboard_omits_unset_fields(client):
    client.configure_dashboard(targets={'loss': 0.1})
    assert _sent_dashboard_rows(client)[0][1] == {'targets': {'loss': 0.1}}


@pytest.mark.parametrize('config, message', [
    ({'stared': ['loss']}, 'unknown dashboard option'),
    ({'starred': 'loss'}, 'list of metric names'),
    ({'targets': {'loss': 'low'}}, 'finite number'),
    ({'targets': {'loss': float('nan')}}, 'finite number'),
    ({'targets': {'loss': True}}, 'finite number'),
])
def test_normalize_dashboard_config_rejects_bad_input(config, message):
    with pytest.raises(ValueError, match=message):
        _normalize_dashboard_config(config)


def test_init_validates_before_connecting():
    with patch('aeroboard_client.client.fl.FlightClient') as flight:
        with pytest.raises(ValueError, match='unknown dashboard option'):
            aeroboard_client.init(run_id='x', dashboard={'stared': ['loss']})
        flight.assert_not_called()


def test_init_applies_dashboard():
    with patch('aeroboard_client.client.fl.FlightClient', DummyFlightClient), \
            patch.object(AeroboardClient, '_resolve_run_id', lambda self, base: base):
        client = aeroboard_client.init(run_id='dash-init', dashboard={'starred': ['loss']})
    assert _sent_dashboard_rows(client)[0][1] == {'starred': ['loss']}


def test_query_hyperparameters_hides_dashboard_entry(client):
    response = MagicMock(status_code=200)
    response.json.return_value = {'run_summaries': [{
        'run_id': 'dash-demo',
        'hyperparameters': [
            {'key': 'lr', 'value': '0.1', 'value_type': 'float'},
            {'key': DASHBOARD_CONFIG_KEY, 'value': '{}', 'value_type': DASHBOARD_CONFIG_VALUE_TYPE},
        ],
    }]}
    client._http_client = MagicMock()
    client._http_client.get.return_value = response
    with patch.object(AeroboardClient, '_ensure_http_client', lambda self: None):
        assert [entry['key'] for entry in client.query_hyperparameters()] == ['lr']
        assert [entry['key'] for entry in client.query_hyperparameters(key=DASHBOARD_CONFIG_KEY)] == [DASHBOARD_CONFIG_KEY]


def test_query_helpers_create_http_session_lazily(client):
    client.ssl_verify = False
    assert client._http_client is None
    client._ensure_http_client()
    session = client._http_client
    assert session is not None and session.verify is False
    client._ensure_http_client()
    assert client._http_client is session
