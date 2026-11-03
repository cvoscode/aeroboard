"""Durability boundaries and failure behavior, independent of a running server."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time

import pytest

from aeroboard_client._outbox import DiskOutbox, OutboxError, OutboxSender
from aeroboard_client.client import AeroboardClient
from aeroboard_client.cli import main
from click.testing import CliRunner


def test_local_commit_survives_abrupt_exit_and_preserves_order(tmp_path):
    script = """
import os,sys
from aeroboard_client import AeroboardClient
c=AeroboardClient('offline',url='grpc://127.0.0.1:1',api_base_url='http://127.0.0.1:1',spool_dir=sys.argv[1],batch_size=2)
c.define_metric('loss',goal='minimize',target=0)
for i in range(20): c.log({'loss':20-i},step=i)
c.log_event('checkpoint',kind='checkpoint',step=19)
assert c.flush(local_only=True)
os._exit(0)
"""
    env = dict(os.environ, PYTHONPATH=str(Path(__file__).resolve().parents[1]/"aeroboard-client/src"))
    subprocess.run([sys.executable,"-c",script,str(tmp_path)],env=env,check=True,timeout=15)
    database = next(tmp_path.rglob("outbox.sqlite"))
    with_outbox = DiskOutbox(database.parent)
    try:
        assert with_outbox.status()["pending_rows"] == 20
        kinds = []
        while (item := with_outbox.next()) is not None:
            kinds.append((item[1],json.loads(item[3])["method"] if item[1]=="control" else item[2]))
            with_outbox.acknowledge(item[0])
        assert kinds[:2] == [("control","BOOTSTRAP"),("control","DEFINE")]
        assert kinds[-1] == ("control","POST")
        assert with_outbox.status()["bytes"] == 0
    finally:
        with_outbox.close()


def test_owner_lock_quota_corruption_and_server_binding_are_inspectable(tmp_path):
    outbox = DiskOutbox(tmp_path,{"version":1},max_bytes=20)
    try:
        with pytest.raises(OutboxError,match="owned"):
            DiskOutbox(tmp_path)
        outbox.bind("first-store")
        with pytest.raises(OutboxError,match="identity changed"):
            outbox.bind("another-store")
        outbox.append("control","",b"original",7)
        with pytest.raises(OutboxError,match="quota"):
            outbox.append("control","",b"x"*21)
        assert outbox.status()["pending_rows"] == 7
        with outbox.db:
            outbox.db.execute("UPDATE items SET body=?",(b"corrupt!",))
        with pytest.raises(OutboxError,match="checksum"):
            outbox.next()
        assert outbox.status()["rejected_batches"] == 1
        assert "checksum" in outbox.status()["errors"][0]
    finally:
        outbox.close()
    # Closing releases ownership for recovery, including after quarantine.
    recovered = DiskOutbox(tmp_path)
    recovered.close()


def test_local_writer_does_not_wait_for_network_and_rejections_block_dependencies(tmp_path):
    outbox = DiskOutbox(tmp_path,{"version":1})
    delivered = []
    def reject(kind,path,body):
        delivered.append(path)
        raise OutboxError("permanently rejected")
    sender = OutboxSender(outbox,reject,on_error=lambda _:None)
    sender.submit_control("POST","first",{})
    sender.submit_control("POST","second",{})
    assert sender.wait_local(2)
    assert not sender.wait_idle(2)
    sender.close(0.1)
    assert delivered == ["first"]
    assert sender.status()["rejected_batches"] == 1
    assert sender.status()["pending_batches"] == 1


def test_sdk_offline_logging_is_fast_and_cli_inspects_without_network(tmp_path):
    client = AeroboardClient("offline",url="grpc://127.0.0.1:1",api_base_url="http://127.0.0.1:1",spool_dir=tmp_path,batch_size=1,flush_timeout=5)
    started = time.monotonic()
    for step in range(100): client.log({"loss":float(step)},step=step)
    assert time.monotonic()-started < 1
    assert client.flush(local_only=True)
    client.close(timeout=0.1)
    result = CliRunner().invoke(main,["sync","--spool-dir",str(tmp_path),"--status"])
    assert result.exit_code == 0, result.output
    assert json.loads(result.output)["pending_rows"] == 100


def test_local_only_requires_persistent_mode():
    client = AeroboardClient("no-spool",background=False)
    try:
        with pytest.raises(ValueError,match="spool_dir"):
            client.flush(local_only=True)
    finally:
        client.close()


def test_disk_full_is_reported_without_evicting_committed_data(tmp_path,monkeypatch):
    outbox=DiskOutbox(tmp_path,{"version":1})
    outbox.append("control","old",b"committed",7)
    def full(*args,**kwargs): raise OSError(28,"No space left on device")
    monkeypatch.setattr(outbox,"append",full)
    sender=OutboxSender(outbox,lambda *args: (_ for _ in ()).throw(ConnectionError("offline")),on_error=lambda _:None)
    sender.submit_control("POST","new",{})
    assert not sender.wait_local(2)
    sender.close(0.1)
    status=sender.status()
    assert status["local_failures"]==1
    assert status["pending_rows"]==7 and status["pending_batches"]==1
