"""Transport shared by live persistent clients and the recovery command."""
from __future__ import annotations

import json
import logging
import pyarrow as pa
import pyarrow.flight as fl

from ._outbox import OutboxError
from ._protocol import ENVELOPE_KEY
from ._tracking import RunTracker


class OutboxTransport:
    def __init__(self, outbox, *, tracker=None, flight=None, on_error=None):
        self.outbox = outbox
        config = outbox.config
        if config.get("version") != 1:
            raise OutboxError("unsupported outbox version")
        self.tracker = tracker or RunTracker(config["api_base_url"], project=config["project"],
            resume=config["resume"], verify=config["ssl_verify"], metadata=config["metadata"],
            tags=config["tags"], on_error=on_error or logging.getLogger(__name__).warning)
        self.tracker.run_uid = config["run_uid"]
        self.tracker.session_id = config["session_id"]
        self.tracker.started_at_ms = config["started_at_ms"]
        self.tracker.segment = config.get("segment")
        self.tracker.replay = tracker is None
        self.flight = flight or fl.FlightClient(config["url"])
        self.owns_flight = flight is None
        self.connected = False

    def connect(self):
        if self.connected:
            return
        self.tracker.connect(self.outbox.config["name"], persistent=True, bind=self.outbox.bind)
        self.outbox.set_setting("canonical_run_id", self.tracker.run_id)
        self.connected = True

    def __call__(self, kind, path, body):
        self.connect()
        if kind == "control":
            operation = json.loads(body)
            if operation["method"] != "BOOTSTRAP":
                self.tracker.deliver(operation["method"], path, operation["body"])
            return
        if kind != "flight":
            raise OutboxError("unsupported outbox operation")
        reader = pa.ipc.open_stream(body)
        batches = list(reader)
        if len(batches) != 1:
            raise OutboxError("outbox entry must contain one Arrow batch")
        batch = batches[0]
        metadata = dict(batch.schema.metadata or {})
        envelope = json.loads(metadata[ENVELOPE_KEY])
        envelope["server_id"] = self.tracker.server_id
        metadata[ENVELOPE_KEY] = json.dumps(envelope).encode()
        batch = batch.replace_schema_metadata(metadata)
        writer, acknowledgements = self.flight.do_put(fl.FlightDescriptor.for_path(path.encode()),
            batch.schema, options=fl.FlightCallOptions(timeout=60))
        try:
            writer.write_batch(batch)
            writer.done_writing()
            if acknowledgements.read() is None:
                raise OutboxError("server did not acknowledge the identified batch")
        finally:
            writer.close()

    def send_many(self,entries):
        self.connect()
        if "batch_streams" not in self.tracker.capabilities.get("features",[]):
            for _ordinal,kind,path,body,_rows in entries:self(kind,path,body)
            return
        batches=[]
        for _ordinal,kind,path,body,_rows in entries:
            if kind!="flight":raise OutboxError("batch stream contains a control operation")
            batch=pa.ipc.open_stream(body).read_next_batch()
            metadata=dict(batch.schema.metadata or {})
            envelope=json.loads(metadata[ENVELOPE_KEY]);envelope["server_id"]=self.tracker.server_id
            metadata[ENVELOPE_KEY]=json.dumps(envelope).encode();batches.append(batch.replace_schema_metadata(metadata))
        if any(not batch.schema.equals(batches[0].schema) for batch in batches):
            for _ordinal,kind,path,body,_rows in entries:self(kind,path,body)
            return
        writer,reader=self.flight.do_put(fl.FlightDescriptor.for_path(entries[0][2].encode()),batches[0].schema,options=fl.FlightCallOptions(timeout=60))
        try:
            for batch in batches:writer.write_with_metadata(batch,batch.schema.metadata[ENVELOPE_KEY])
            writer.done_writing()
            if reader.read() is None:raise OutboxError("server did not acknowledge the batch stream")
        finally:writer.close()

    def cancel(self):
        # Prevent an in-flight handshake from starting another HTTP operation
        # after the sender's delivery deadline. The current call is bounded.
        self.tracker.close()

    def close(self):
        self.cancel()
        if self.owns_flight:
            self.flight.close()
