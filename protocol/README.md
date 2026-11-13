# Aeroboard telemetry protocol

A Flight descriptor names `scalars`, `hyperparameters`, `images`, `audio`, or
`video`. All batches begin with `timestamp: timestamp[ms]` and `run_id: utf8`.
Scalars add `name: utf8`, `value: float64`, `step: float64?`. Hyperparameters add
`key: utf8`, `value: utf8`, `value_type: utf8`. Media adds `name: utf8`, a binary
`image_data`/`audio_data`/`video_data` column, and `format: utf8`; identified media
also includes `asset_id: utf8`, `step: float64?`, `segment: utf8?`.

The schema metadata key `aeroboard.envelope` contains JSON with `version`,
`batch_id`, `run_uid`, `session_id`, `server_id`, `sequence`, `checksum`,
`captured_at_ms`, and optional `segment`. IDs are UUIDs; sequences start at one
per session. A duplicate identity and checksum is acknowledged without replaying
its live update or updating counters. Reusing an ID or sequence is rejected.
An acknowledgment means the payload and its catalog receipt are committed.

Multiple batches can share a Flight RPC. For distinct envelopes on the same
schema, send each envelope as the batch's Flight application metadata. The final
acknowledgment contains the total accepted row count; retrying the stream is safe.

## Checksums

Both versions use SHA-256 and little-endian integer/IEEE-754 values. Schema
metadata, field nullability, array offsets, and padding are excluded. Field order
is significant. Type tags are `s` (utf8), `b` (binary), `f` (float64), `t`
(timestamp milliseconds). NaN is always `0x7ff8000000000000`; negative zero is
preserved. A timestamp with a different unit is rejected by schema validation.

Version 1 starts with the uint64 row count. For each column: uint32 UTF-8 field
name length, field name, one-byte type tag. Then each row: byte 0 for null; byte 1
followed by the value otherwise. Variable values have a uint64 byte length before
their bytes. Numbers occupy eight bytes.

Version 2 starts with the bytes `aeroboard-columnar-v2\0`, then the uint64 row
count. Each column starts with the same name and type header. It is followed by
one validity byte per row (0 or 1), then canonical column buffers. Numeric buffers
use eight bytes per row, zero for nulls. Variable columns use uint32 offsets
starting at zero (row count + 1 offsets), followed by concatenated UTF-8/binary
bytes; nulls contribute no bytes. V2 permits bulk hashing on both implementations.
The SDK negotiates v2 using `protocol_version`; v1 remains supported.

`checksum-vectors.json` is consumed by the Rust and Python test suites and covers
Unicode, empty columns, sliced buffers, nulls, NaN, infinity, and negative zero.

## Live subscriptions

Send `{"type":"subscribe","runs":["canonical-run-id"]}` after connecting and
whenever the selection changes. Binary Arrow messages contain selected runs.
`data_changed` messages list activity across runs; `*_changed` catalog messages
include entity kind, ID, and revision. `lagged` requests a history resync.
Persistent media pushes contain empty payloads; fetch bytes by asset ID.
