"""Media conversion; encoding runs on the delivery worker."""
import io
import os
import wave
from pathlib import Path
from typing import Any, Optional
MAX_IMAGE_BYTES=256*1024
MAX_MEDIA_BYTES=32*1024*1024
_MEDIA_DEFAULT_FORMATS={"audio":"wav","video":"mp4"}
def _coerce_image_bytes(
    image: Any,
    *,
    format: Optional[str] = None,
    max_bytes: int = MAX_IMAGE_BYTES,
) -> tuple[bytes, str]:
    """
    Coerce image data to bytes and infer format.

    Args:
        image: Image in any supported format (bytes, PIL, numpy, torch).
        format: Target format (default: inferred).
        max_bytes: Maximum allowed size in bytes.

    Returns:
        Tuple of (encoded_bytes, format_string).
    """
    inferred_format = (format or ("png" if isinstance(image,(bytes,bytearray,memoryview)) else "webp")).lower()
    if isinstance(image, (bytes, bytearray, memoryview)):
        raw = bytes(image)
        return _maybe_compress_encoded_image(raw, inferred_format, max_bytes=max_bytes)

    try:
        from PIL import Image
    except ImportError:
        Image = None

    if Image is not None and isinstance(image, Image.Image):
        return _encode_pil_image(image, inferred_format, max_bytes=max_bytes)

    try:
        import numpy as np
    except ImportError:
        np = None

    if np is not None and isinstance(image, np.ndarray):
        if Image is None:
            raise RuntimeError("Pillow is required to encode numpy array images")
        pil_image = Image.fromarray(image)
        return _coerce_image_bytes(pil_image, format=inferred_format, max_bytes=max_bytes)

    try:
        import torch
    except ImportError:
        torch = None

    if torch is not None and isinstance(image, torch.Tensor):
        if np is None:
            raise RuntimeError("numpy is required to encode torch tensor images")
        arr = image.detach().cpu().numpy()
        if arr.ndim == 3 and arr.shape[0] in (1, 3, 4):
            arr = arr.transpose(1, 2, 0)
        return _coerce_image_bytes(arr, format=inferred_format, max_bytes=max_bytes)

    raise TypeError("Unsupported image type. Use bytes, PIL Image, numpy array, or torch tensor.")


def _normalize_media_format(value: str) -> str:
    return value.strip().lstrip(".").lower()


def _coerce_encoded_media(
    media: Any,
    *,
    kind: str,
    format: Optional[str] = None,
) -> tuple[bytes, str]:
    """
    Accept already-encoded media as bytes or a file path. No transcoding is done.

    Returns:
        Tuple of (payload, format).
    """
    if isinstance(media, (bytes, bytearray, memoryview)):
        media_format = _normalize_media_format(format or _MEDIA_DEFAULT_FORMATS[kind])
        return bytes(media), media_format

    if isinstance(media, (str, os.PathLike)):
        path = os.fspath(media)
        suffix = os.path.splitext(path)[1]
        media_format = _normalize_media_format(format or suffix or _MEDIA_DEFAULT_FORMATS[kind])
        with open(path, "rb") as handle:
            return handle.read(), media_format

    hint = (
        " Encode frames to MP4/WebM first (e.g. with ffmpeg) and pass the bytes or file path."
        if kind == "video"
        else ""
    )
    raise TypeError(f"Unsupported {kind} type {type(media).__name__}. Use bytes or a file path.{hint}")


def _coerce_audio_bytes(
    audio: Any,
    *,
    sample_rate: Optional[int] = None,
    format: Optional[str] = None,
) -> tuple[bytes, str]:
    """
    Coerce audio to encoded bytes.

    Encoded bytes and file paths pass through unchanged. numpy arrays and torch
    tensors are packed as 16-bit PCM WAV (vectorised numpy + stdlib ``wave``).
    """
    if isinstance(audio, (bytes, bytearray, memoryview, str, os.PathLike)):
        return _coerce_encoded_media(audio, kind="audio", format=format)

    if hasattr(audio, "detach") and hasattr(audio, "cpu"):  # torch.Tensor without importing torch
        audio = audio.detach().cpu().numpy()

    try:
        import numpy as np
    except ImportError:
        np = None

    if np is None or not isinstance(audio, np.ndarray):
        raise TypeError(
            f"Unsupported audio type {type(audio).__name__}. Use bytes, a file path, a numpy array or a torch tensor."
        )

    if format is not None and _normalize_media_format(format) != "wav":
        raise ValueError("Waveform arrays are encoded as WAV; pass encoded bytes to log other formats.")
    if not sample_rate or sample_rate <= 0:
        raise ValueError("sample_rate is required when logging a waveform array.")

    return _encode_wav(audio, int(sample_rate)), "wav"


def _encode_wav(samples: Any, sample_rate: int) -> bytes:
    import numpy as np

    arr = np.asarray(samples)
    if arr.ndim == 2 and arr.shape[0] < arr.shape[1]:
        arr = arr.T  # (channels, samples) -> (samples, channels)
    if arr.ndim == 1:
        arr = arr[:, None]
    if arr.ndim != 2 or arr.shape[0] == 0:
        raise ValueError(f"Expected a 1D or 2D waveform, got shape {np.shape(samples)}.")

    if np.issubdtype(arr.dtype, np.floating):
        pcm = (np.clip(np.nan_to_num(arr), -1.0, 1.0) * 32767.0).astype("<i2")
    elif arr.dtype == np.int16:
        pcm = arr.astype("<i2", copy=False)
    else:
        raise TypeError(f"Unsupported waveform dtype {arr.dtype}; use float in [-1, 1] or int16.")

    with io.BytesIO() as buffer:
        with wave.open(buffer, "wb") as writer:
            writer.setnchannels(pcm.shape[1])
            writer.setsampwidth(2)
            writer.setframerate(sample_rate)
            writer.writeframes(np.ascontiguousarray(pcm).tobytes())
        return buffer.getvalue()


def _maybe_compress_encoded_image(
    payload: bytes,
    image_format: str,
    *,
    max_bytes: int = MAX_IMAGE_BYTES,
) -> tuple[bytes, str]:
    """
    Compress large images to stay within size limits.

    Args:
        payload: Raw image bytes.
        image_format: Image format.
        max_bytes: Maximum allowed size.

    Returns:
        Compressed payload and format (may be converted to webp).
    """
    if len(payload) <= max_bytes:
        return payload, image_format

    try:
        from PIL import Image
    except ImportError:
        return payload, image_format

    try:
        with Image.open(io.BytesIO(payload)) as image:
            converted = (
                image.convert('RGB')
                if image.mode not in ('RGB', 'L')
                else image.copy()
            )
            return _encode_pil_image(converted, 'webp', max_bytes=max_bytes)
    except Exception:
        return payload, image_format


def _encode_pil_image(
    image: Any,  # PIL Image
    image_format: str,
    *,
    max_bytes: int = MAX_IMAGE_BYTES,
) -> tuple[bytes, str]:
    """
    Encode a PIL image to bytes.

    Args:
        image: PIL Image object.
        image_format: Target format.
        max_bytes: Maximum allowed size.

    Returns:
        Encoded bytes and format.
    """
    # Check if image is actually a PIL Image
    if not hasattr(image, 'save'):
        raise TypeError(f"Expected PIL Image, got {type(image)}")

    normalized = image_format.lower()
    save_format = 'JPEG' if normalized in {'jpg', 'jpeg'} else normalized.upper()

    if save_format == 'WEBP':
        for quality in (85, 75, 60):
            with io.BytesIO() as buffer:
                image.save(buffer, format='WEBP', quality=quality, method=4)
                payload = buffer.getvalue()
            if len(payload) <= max_bytes:
                return payload, 'webp'
        return payload, 'webp'

    with io.BytesIO() as buffer:
        image.save(buffer, format=save_format)
        payload = buffer.getvalue()

    if len(payload) > max_bytes:
        rgb_image = (
            image.convert('RGB')
            if getattr(image, 'mode', '') not in ('RGB', 'L')
            else image
        )
        return _encode_pil_image(rgb_image, 'webp', max_bytes=max_bytes)

    return payload, normalized


def snapshot_image(image):
    if isinstance(image,(bytes,bytearray,memoryview)):
        return bytes(image),len(image)
    if hasattr(image,"detach") and hasattr(image,"cpu"):
        image=image.detach().cpu().numpy().copy()
        if image.ndim==3 and image.shape[0] in (1,3,4):image=image.transpose(1,2,0)
        return image,image.nbytes
    if hasattr(image,"save") and hasattr(image,"copy"):
        return image.copy(),image.width*image.height*len(image.getbands())
    if hasattr(image,"dtype") and hasattr(image,"nbytes") and hasattr(image,"copy"):
        return image.copy(),image.nbytes
    raise TypeError("Unsupported image type. Use bytes, PIL Image, numpy array, or torch tensor.")
