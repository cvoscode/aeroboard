"""Opt-in real HTTP/Flight tests. Set AEROBOARD_TEST_BINARY; optionally GREPTIME_TEST_URL."""
import csv
import io
import json
import os
from pathlib import Path
import socket
import sqlite3
import subprocess
import sys
import time
import uuid

import pytest
import pyarrow as pa
import requests

from aeroboard_client import AeroboardClient
from aeroboard_client._delivery import OutboxTransport
from aeroboard_client._outbox import DiskOutbox
from aeroboard_client.cli import main
from click.testing import CliRunner

pytestmark = pytest.mark.skipif(not os.getenv("AEROBOARD_TEST_BINARY"),reason="requires a built server and local sockets")


def port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1",0))
        return sock.getsockname()[1]


@pytest.fixture(params=["filesystem","greptime"])
def server(request,tmp_path):
    if request.param=="greptime" and not os.getenv("GREPTIME_TEST_URL"):
        pytest.skip("GREPTIME_TEST_URL is unset")
    http,flight=port(),port()
    root=tmp_path/"server"
    env=dict(os.environ,AEROBOARD_HTTP_PORT=str(http),AEROBOARD_FLIGHT_PORT=str(flight),
        AEROBOARD_DATA_DIR=str(root),AEROBOARD_STORAGE=request.param,
        AEROBOARD_GREPTIME_URL=os.getenv("GREPTIME_TEST_URL","http://127.0.0.1:14000"))
    log=open(tmp_path/"server.log","w+")
    process=subprocess.Popen([os.environ["AEROBOARD_TEST_BINARY"]],env=env,cwd=tmp_path,stdout=log,stderr=log)
    base=f"http://127.0.0.1:{http}"
    def stop():
        process.terminate()
        try: process.wait(timeout=5)
        except subprocess.TimeoutExpired: process.kill();process.wait()
    def restart():
        nonlocal process
        stop()
        process=subprocess.Popen([os.environ["AEROBOARD_TEST_BINARY"]],env=env,cwd=tmp_path,stdout=log,stderr=log)
        for _ in range(200):
            try:
                if requests.get(base+"/api/capabilities",timeout=0.2).ok: return
            except requests.RequestException: pass
            time.sleep(0.05)
        pytest.fail("server did not restart")
    try:
        for _ in range(200):
            try:
                if requests.get(base+"/api/capabilities",timeout=0.2).ok: break
            except requests.RequestException: pass
            if process.poll() is not None:
                log.seek(0);pytest.fail(log.read())
            time.sleep(0.05)
        else: pytest.fail("server did not become ready")
        yield base,f"grpc://127.0.0.1:{flight}",root,restart
    finally:
        stop()
        log.close()


def api(base,method,path,**kwargs):
    response=requests.request(method,base+"/api"+path,timeout=15,**kwargs)
    assert response.ok,(response.status_code,response.text)
    return response.json()


def test_replay_fault_windows_scores_assets_views_filters_and_tombstones(server,tmp_path):
    base,flight,root,restart=server
    project="test_"+uuid.uuid4().hex
    # Capture every dependency and media byte with no server contact.
    client=AeroboardClient("trial",url=flight,api_base_url=base,project=project,spool_dir=tmp_path/"spool",batch_size=100,flush_timeout=5)
    client._transport.connect=lambda: (_ for _ in ()).throw(ConnectionError("offline fixture"))
    client.define_metric("val/loss",goal="minimize",target=0,unit="fraction")
    client.log_hyperparameters({"lr":0.01,"seed":1})
    client.log({"val/loss":1.0},step=0)
    client.log({"val/loss":0.0},step=1)
    a=client.log_video("clip",b"first",format="mp4",timestamp_ms=1234,step=1)
    b=client.log_video("clip",b"second",format="mp4",timestamp_ms=1234,step=1)
    event=client.log_event("checkpoint",kind="checkpoint",step=1,media_ids=[a,b])
    uid=client.run_uid
    client.finish()
    database=next((tmp_path/"spool").rglob("outbox.sqlite"))
    outbox=DiskOutbox(database.parent)
    transport=OutboxTransport(outbox)
    replayed=[]
    storage=api(base,"GET","/capabilities")["storage"]
    try:
        while (item:=outbox.next()) is not None:
            ordinal,kind,path,body,_=item
            transport(kind,path,body)
            if kind=="flight":
                # Lost acknowledgement: submitting identical bytes has one effect.
                transport(kind,path,body)
                # A new identity cannot reuse the acknowledgement of this batch.
                batch=pa.ipc.open_stream(body).read_next_batch()
                envelope=json.loads(batch.schema.metadata[b"aeroboard.envelope"])
                envelope["captured_at_ms"]+=1
                changed=batch.replace_schema_metadata({**batch.schema.metadata,b"aeroboard.envelope":json.dumps(envelope).encode()})
                sink=pa.BufferOutputStream()
                with pa.ipc.new_stream(sink,changed.schema) as writer: writer.write_batch(changed)
                with pytest.raises(pa.ArrowInvalid): transport(kind,path,sink.getvalue().to_pybytes())
                # Backend write survived but the receipt commit did not.
                with sqlite3.connect(root/"catalog.sqlite") as db:
                    batch_id=json.loads(pa.ipc.open_stream(body).schema.metadata[b"aeroboard.envelope"])["batch_id"]
                    db.execute("UPDATE receipts SET committed=0 WHERE id=?",(batch_id,))
                transport(kind,path,body)
                # Pending receipt with no backend effect: replay must perform
                # the write again, then make it visible exactly once.
                with sqlite3.connect(root/"catalog.sqlite") as db:
                    db.execute("UPDATE receipts SET committed=0 WHERE id=?",(batch_id,))
                if storage=="filesystem":
                    (root/"batches"/f"{batch_id}.arrow").unlink()
                else:
                    response=requests.post(os.environ["GREPTIME_TEST_URL"]+"/v1/sql",data={"sql":f"DELETE FROM aeroboard_batches WHERE batch_id='{batch_id}'"},timeout=10)
                    response.raise_for_status();assert not response.json().get("error")
                transport(kind,path,body)
            replayed.append((kind,path,body))
            outbox.acknowledge(ordinal)
    finally:
        transport.close();outbox.close()
    store_id=api(base,"GET","/capabilities")["server_id"]
    restart()
    assert api(base,"GET","/capabilities")["server_id"]==store_id
    run=api(base,"GET",f"/run-records/{uid}")
    name=run["data"]["run_id"]
    assert run["data"]["status"]=="completed"
    assert run["data"]["last_heartbeat_ms"] is None
    assert run["data"]["delivery"]["status"]=="delivered"
    history=api(base,"GET","/history",params={"run_id":name})["rows"]
    assert len(history)==2
    scores=api(base,"GET","/scorecards",params={"run_id":name})["rows"]
    assert scores[0]["best"]==0 and scores[0]["finite_count"]==2
    assert scores[0]["best_step"]==1 and scores[0]["target_at_ms"] is not None
    clips=api(base,"GET","/history/media",params={"run_id":name,"kind":"video"})["rows"]
    assert {clip["asset_id"] for clip in clips}=={a,b}
    for asset,payload in [(a,b"first"),(b,b"second")]:
        response=requests.get(base+f"/api/assets/{asset}/content",headers={"Range":"bytes=0-1"},timeout=10)
        assert response.status_code==206 and response.content==payload[:2]
    events=api(base,"GET","/events",params={"run_id":uid})["rows"]
    assert len(events)==1 and events[0]["id"]==event
    api(base,"PATCH",f"/events/{event}",json={"revision":1,"label":"Edited checkpoint"})
    # Replayed original event remains idempotent after an annotation edit.
    event_op=next(item for item in replayed if item[0]=="control" and item[1]=="/api/events")
    recovered=DiskOutbox(database.parent);transport=OutboxTransport(recovered)
    try: transport(*event_op)
    finally: transport.close();recovered.close()
    assert api(base,"GET","/events",params={"run_id":uid})["rows"][0]["data"]["label"]=="Edited checkpoint"
    filters=[{"field":"params.lr","op":"lte","value":0.01},{"field":"metrics.val/loss.best","op":"eq","value":0}]
    filtered=api(base,"GET","/run-records",params={"project":project,"filters":json.dumps(filters)})
    assert [r["id"] for r in filtered["rows"]]==[uid]
    view=api(base,"POST",f"/projects/{project}/views",json={"name":"Focus","settings":{"dashboard":{"starred":["val/loss"],"targets":{"val/loss":0}},"removedTargets":["other"],"selectedRuns":[name]}})
    stale=requests.patch(base+f"/api/views/{view['id']}",json={"revision":0,"name":"stale","settings":{}},timeout=10)
    assert stale.status_code==409
    aggregation=api(base,"POST","/comparison/aggregate",json={"run_ids":[uid],"metric":"val/loss"})
    assert [p["n"] for p in aggregation["groups"][0]["points"]]==[1,1]
    assert all(p["stddev"] is None for p in aggregation["groups"][0]["points"])
    export=requests.get(base+"/api/export/scalars.csv",params={"run_id":name},timeout=10)
    assert export.ok and len(list(csv.DictReader(io.StringIO(export.text))))==2
    api(base,"DELETE",f"/run-records/{uid}",params={"revision":1})
    assert api(base,"GET","/history",params={"run_id":name})["rows"]==[]
    # A stopped SDK's delayed batches cannot recreate deleted generations.
    recovered=DiskOutbox(database.parent);transport=OutboxTransport(recovered)
    try:
        with pytest.raises(Exception): transport(*next(item for item in replayed if item[0]=="flight"))
    finally: transport.close();recovered.close()
    assert name not in api(base,"GET","/runs")["runs"]


def test_live_sdk_lifecycle_resume_and_recovery_cli(server,tmp_path):
    base,flight,_,_=server
    with AeroboardClient("live_"+uuid.uuid4().hex,url=flight,api_base_url=base,spool_dir=tmp_path/"spool") as client:
        uid=client.run_uid
        client.define_metric("accuracy",goal="maximize",target=0.9)
        client.log({"accuracy":0.5},step=1)
        assert client.flush()
        assert api(base,"GET","/scorecards",params={"run_id":uid})["rows"][0]["best"]==0.5
        client.log({"accuracy":0.95},step=2)
        assert client.flush()
        scores=api(base,"GET","/scorecards",params={"run_id":uid})["rows"]
        assert scores[0]["best"]==0.95 and scores[0]["finite_count"]==2
    assert api(base,"GET",f"/run-records/{uid}")["data"]["status"]=="completed"
    with AeroboardClient(uid,url=flight,api_base_url=base,resume=True) as resumed:
        resumed.log({"accuracy":0.99},step=3)
    run=api(base,"GET",f"/run-records/{uid}")
    assert run["data"]["attempt"]==2
    assert api(base,"GET","/scorecards",params={"run_id":uid})["rows"][0]["finite_count"]==3
    with AeroboardClient(uid,url=flight,api_base_url=base,resume=True,segment="restart") as resumed:
        resumed.log({"accuracy":0.4},step=0)
        resumed.log({"accuracy":0.8},step=1)
    mixed=api(base,"GET","/scorecards",params={"run_id":uid,"step":1})["rows"][0]
    assert mixed["at_step"] is None
    segmented=api(base,"GET","/scorecards",params={"run_id":uid,"step":1,"segment":"restart"})["rows"][0]
    assert segmented["at_step"]==0.8
    groups=api(base,"POST","/comparison/aggregate",json={"run_ids":[uid],"metric":"accuracy"})["groups"]
    assert len(groups)==2
    result=CliRunner().invoke(main,["sync","--spool-dir",str(tmp_path/"spool")])
    assert result.exit_code==0,result.output


def test_abrupt_process_exit_recovers_through_cli_without_fresh_heartbeat(server,tmp_path):
    base,flight,_,_=server
    spool=tmp_path/"crash-spool"
    script="""
import os,sys
from aeroboard_client import AeroboardClient
c=AeroboardClient('crashed',url=sys.argv[1],api_base_url=sys.argv[2],spool_dir=sys.argv[3])
c._transport.connect=lambda: (_ for _ in ()).throw(ConnectionError('offline fixture'))
for step in range(200): c.log({'loss':step},step=step)
assert c.flush(local_only=True)
os._exit(0)
"""
    env=dict(os.environ,PYTHONPATH=str(Path(__file__).resolve().parents[1]/"aeroboard-client/src"))
    subprocess.run([sys.executable,"-c",script,flight,base,str(spool)],env=env,check=True,timeout=20)
    database=next(spool.rglob("outbox.sqlite"));outbox=DiskOutbox(database.parent)
    uid=outbox.config["run_uid"];outbox.close()
    result=CliRunner().invoke(main,["sync","--spool-dir",str(spool)])
    assert result.exit_code==0,result.output
    run=api(base,"GET",f"/run-records/{uid}")
    assert run["data"]["display_status"]=="stale"
    assert run["data"]["last_heartbeat_ms"] is None
    assert len(api(base,"GET","/history",params={"run_id":run["data"]["run_id"]})["rows"])==200


def test_aggregation_statistics_missing_coverage_and_objective_revision(server):
    base,flight,_,_=server
    project="stats_"+uuid.uuid4().hex
    ids=[]
    for seed,values in [(1,[(0,1.0),(1,2.0)]),(2,[(0,3.0),(2,5.0)])]:
        with AeroboardClient("stats",project=project,url=flight,api_base_url=base) as run:
            ids.append(run.run_uid)
            run.define_metric("loss",goal="minimize",target=2.0)
            run.log_hyperparameters({"seed":seed,"lr":0.01})
            for step,value in values: run.log({"loss":value},step=step)
    groups=api(base,"POST","/comparison/aggregate",json={"run_ids":ids,"metric":"loss"})["groups"]
    assert len(groups)==1
    rows=groups[0]["points"]
    assert rows[0]["mean"]==2 and rows[0]["stddev"]==pytest.approx(2**0.5) and rows[0]["n"]==2
    assert [row["n"] for row in rows]==[2,1,1]
    assert rows[1]["stddev"] is None
    assert api(base,"GET","/scorecards",params={"run_id":ids[0]})["rows"][0]["best"]==1
    definition=api(base,"GET","/metric-definitions",params={"parent":ids[0]})[0]
    api(base,"POST","/metric-definitions",json={"parent":ids[0],"name":"loss","revision":definition["revision"],"goal":"maximize","target":3})
    score=api(base,"GET","/scorecards",params={"run_id":ids[0]})["rows"][0]
    assert score["best"]==2 and score["target_at_ms"] is None and score["finite_count"]==2
    # Incompatible objective definitions must form separate populations.
    assert len(api(base,"POST","/comparison/aggregate",json={"run_ids":ids,"metric":"loss"})["groups"])==2


def test_pagination_and_csv_export_include_every_page(server):
    base,flight,_,_=server
    with AeroboardClient("export_"+uuid.uuid4().hex,url=flight,api_base_url=base,batch_size=1000) as run:
        name=run.run_id
        for step in range(5001): run.log({"loss":float(step)},step=step)
    cursor=0;rows=[]
    while cursor is not None:
        page=api(base,"GET","/history",params={"run_id":name,"limit":1000,"cursor":cursor,"include_summaries":"false"})
        rows.extend(page["rows"]);cursor=page["next_cursor"]
    assert len(rows)==5001
    response=requests.get(base+"/api/export/scalars.csv",params={"run_id":name},timeout=30)
    assert response.ok
    exported=list(csv.DictReader(io.StringIO(response.text)))
    assert len(exported)==5001
    assert {int(float(row["step"])) for row in exported}==set(range(5001))
