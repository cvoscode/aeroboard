import io
import wave
from unittest.mock import patch

import pytest

pytestmark = pytest.mark.legacy_server

import aeroboard_client
import aeroboard_client.client as client_module
from aeroboard_client.client import AeroboardClient, _coerce_audio_bytes, _coerce_encoded_media

from test_logging_payloads import DummyFlightClient


@pytest.fixture
def media_client():
    with patch('aeroboard_client.client.fl.FlightClient', DummyFlightClient), \
            patch.object(AeroboardClient, '_resolve_run_id', lambda self, base: base):
        yield AeroboardClient(run_id='media-demo', batch_size=400)


def _read_wav(payload: bytes):
    with wave.open(io.BytesIO(payload), 'rb') as reader:
        return reader.getnchannels(), reader.getframerate(), reader.getsampwidth(), reader.readframes(reader.getnframes())


def test_encoded_media_bytes_pass_through_untouched():
    payload, fmt = _coerce_encoded_media(b'\x00mp4-bytes', kind='video')
    assert payload == b'\x00mp4-bytes'
    assert fmt == 'mp4'

    payload, fmt = _coerce_encoded_media(memoryview(b'ogg'), kind='audio', format='.OGG')
    assert payload == b'ogg'
    assert fmt == 'ogg'


def test_encoded_media_reads_file_and_infers_format(tmp_path):
    clip = tmp_path / 'episode.webm'
    clip.write_bytes(b'webm-data')
    assert _coerce_encoded_media(clip, kind='video') == (b'webm-data', 'webm')
    assert _coerce_encoded_media(str(clip), kind='video', format='mkv') == (b'webm-data', 'mkv')


def test_video_rejects_raw_frames_with_hint():
    np = pytest.importorskip('numpy')
    with pytest.raises(TypeError, match='ffmpeg'):
        _coerce_encoded_media(np.zeros((2, 4, 4, 3), dtype=np.uint8), kind='video')


def test_audio_float_waveform_encodes_to_pcm16_wav():
    np = pytest.importorskip('numpy')
    waveform = np.array([0.0, 1.0, -1.0, 2.0, np.nan], dtype=np.float32)
    payload, fmt = _coerce_audio_bytes(waveform, sample_rate=8000)

    assert fmt == 'wav'
    channels, rate, width, frames = _read_wav(payload)
    assert (channels, rate, width) == (1, 8000, 2)
    assert np.frombuffer(frames, dtype='<i2').tolist() == [0, 32767, -32767, 32767, 0]


def test_audio_channels_first_is_transposed_to_interleaved():
    np = pytest.importorskip('numpy')
    stereo = np.array([[1, 2, 3], [-1, -2, -3]], dtype=np.int16)  # (channels, samples)
    payload, _ = _coerce_audio_bytes(stereo, sample_rate=16000)

    channels, _, _, frames = _read_wav(payload)
    assert channels == 2
    assert np.frombuffer(frames, dtype='<i2').tolist() == [1, -1, 2, -2, 3, -3]


def test_audio_tensor_like_objects_are_supported_without_torch_import():
    np = pytest.importorskip('numpy')

    class FakeTensor:
        def detach(self):
            return self

        def cpu(self):
            return self

        def numpy(self):
            return np.zeros(4, dtype=np.float32)

    payload, fmt = _coerce_audio_bytes(FakeTensor(), sample_rate=22050)
    assert fmt == 'wav'
    assert _read_wav(payload)[1] == 22050


def test_audio_waveform_validation():
    np = pytest.importorskip('numpy')
    with pytest.raises(ValueError, match='sample_rate'):
        _coerce_audio_bytes(np.zeros(4), sample_rate=None)
    with pytest.raises(ValueError, match='WAV'):
        _coerce_audio_bytes(np.zeros(4), sample_rate=8000, format='mp3')
    with pytest.raises(TypeError, match='dtype'):
        _coerce_audio_bytes(np.zeros(4, dtype=np.int64), sample_rate=8000)
    with pytest.raises(TypeError):
        _coerce_audio_bytes(object())


def test_log_audio_and_video_flush_with_kind_descriptors(media_client):
    media_client.log_audio('eval/speech', b'RIFF-wav', timestamp_ms=1000)
    media_client.log_video('eval/rollout', b'mp4-bytes', timestamp_ms=2000)
    assert media_client.client.calls == []  # buffered until flush

    media_client.flush()

    calls = {descriptor.path[0]: (schema, writer) for descriptor, schema, writer in media_client.client.calls}
    assert set(calls) == {b'audio', b'video'}

    audio_schema, audio_writer = calls[b'audio']
    assert audio_schema.names == ['timestamp', 'run_id', 'name', 'audio_data', 'format']
    audio_row = audio_writer.batches[0].to_pylist()[0]
    assert audio_row['audio_data'] == b'RIFF-wav'
    assert audio_row['format'] == 'wav'
    assert audio_row['run_id'] == 'media-demo'

    video_schema, video_writer = calls[b'video']
    assert video_schema.names == ['timestamp', 'run_id', 'name', 'video_data', 'format']
    assert video_writer.batches[0].to_pylist()[0]['video_data'] == b'mp4-bytes'

    assert media_client._media_buffers == {'audio': [], 'video': []}
    assert media_client._media_buffer_bytes == 0


def test_media_flushes_once_buffered_bytes_exceed_threshold(media_client, monkeypatch):
    monkeypatch.setattr(client_module, 'MEDIA_FLUSH_BYTES', 10)
    media_client.log_video('clip', b'12345')
    assert media_client.client.calls == []
    media_client.log_video('clip', b'678901')
    media_client.flush()
    assert len(media_client.client.calls) == 1
    assert media_client.client.calls[0][2].batches[0].num_rows == 2


def test_media_size_and_empty_limits(media_client, monkeypatch):
    monkeypatch.setattr(client_module, 'MAX_MEDIA_BYTES', 4)
    with pytest.raises(ValueError, match='limit'):
        media_client.log_video('clip', b'too-large')
    with pytest.raises(ValueError, match='empty'):
        media_client.log_audio('clip', b'')
    assert media_client._media_buffers == {'audio': [], 'video': []}


def test_module_level_media_helpers():
    with patch('aeroboard_client.client.fl.FlightClient', DummyFlightClient), \
            patch.object(AeroboardClient, '_resolve_run_id', lambda self, base: base):
        client = aeroboard_client.init(run_id='helpers', batch_size=4)  # flushes every clip
        aeroboard_client.log_audio('a', b'wav')
        aeroboard_client.log_video('v', b'mp4', format='mp4')
        client.flush()

        assert [call[0].path[0] for call in client.client.calls] == [b'audio', b'video']
