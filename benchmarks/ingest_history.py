"""Isolated ingestion/history benchmark; never modifies an existing workspace.

uv run benchmarks/ingest_history.py --binary aeroboard-server/target/release/aeroboard
Use --baseline-binary to run the same cases against a previous build. Reports are
JSON; --browser also measures navigation-to-chart with the current dashboard.
"""
import argparse
import json
import os
from pathlib import Path
import socket
import sqlite3
import subprocess
import tempfile
import time
import uuid

import pyarrow as pa
import requests
from aeroboard_client import AeroboardClient


def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1',0))
        return sock.getsockname()[1]


def seed_receipts(root,count):
    with sqlite3.connect(root/'catalog.sqlite') as db:
        fields={row[1] for row in db.execute('PRAGMA table_info(receipts)')}
        columns=['id','checksum','run_uid','sequence','body','committed']
        if 'session_id' in fields:columns+=['session_id','kind']
        manifest=json.dumps({'run_uid':'benchmark-background','kind':'scalars','envelope':{'session_id':'background'}}).encode()
        def rows():
            for i in range(count):
                row=[f'benchmark-{i}','seed','benchmark-background',i,manifest,0]
                yield row+(['background','scalars'] if 'session_id' in fields else [])
        db.executemany(f"INSERT INTO receipts({','.join(columns)}) VALUES({','.join('?' for _ in columns)})",rows())


def case(binary,receipts,batch_size,points,history=False,browser=False):
    with tempfile.TemporaryDirectory(prefix='aeroboard-perf-') as directory:
        root=Path(directory);http,flight=free_port(),free_port()
        api=f'http://127.0.0.1:{http}'
        log=(root/'server.log').open('w+')
        env=dict(os.environ,AEROBOARD_STORAGE='filesystem',AEROBOARD_DATA_DIR=str(root/'data'),
            AEROBOARD_HTTP_PORT=str(http),AEROBOARD_FLIGHT_PORT=str(flight),
            AEROBOARD_WEB_DIST_DIR=str(Path('aeroboard-web/dist').resolve()))
        process=subprocess.Popen([str(binary.resolve())],env=env,stdout=log,stderr=log)
        try:
            deadline=time.monotonic()+30
            while True:
                try:
                    response=requests.get(api+'/api/capabilities',timeout=.2);response.raise_for_status();capabilities=response.json();break
                except requests.RequestException:
                    if process.poll() is not None or time.monotonic()>deadline:
                        log.seek(0);raise RuntimeError(log.read())
                    time.sleep(.05)
            seed_receipts(root/'data',receipts)
            client=AeroboardClient('benchmark-'+uuid.uuid4().hex,url=f'grpc://127.0.0.1:{flight}',api_base_url=api,
                batch_size=batch_size,flush_timeout=120,max_queue_bytes=512*1024**2)
            client.configure_dashboard(starred=['loss'])
            name=client.run_id
            started=time.perf_counter()
            for step in range(points):client.log({'loss':float(step%100)},step=step)
            if not client.flush() or client.had_error():raise RuntimeError('benchmark upload failed')
            elapsed=time.perf_counter()-started
            client.finish();client.close()
            report={'binary':str(binary),'receipts':receipts,'batch_size':batch_size,'points':points,'seconds':elapsed,'points_per_second':points/elapsed}
            if history:
                for label,max_points in [('full',None),('overview',10000)]:
                    tick=time.perf_counter()
                    response=requests.get(api+'/api/history',params={'run_id':name,'format':'arrow',**({'max_points':max_points} if max_points else {})},timeout=120)
                    response.raise_for_status();server_ms=(time.perf_counter()-tick)*1000
                    tick=time.perf_counter();table=pa.ipc.open_stream(response.content).read_all();decode_ms=(time.perf_counter()-tick)*1000
                    assert table.num_rows==(points if max_points is None else min(points,table.num_rows))
                    if max_points is not None and capabilities.get('protocol_version',1)>=2:assert 0<table.num_rows<=max_points
                    report[label]={'http_ms':server_ms,'decode_ms':decode_ms,'bytes':len(response.content),'rows':table.num_rows}
                if browser:
                    result=subprocess.run(['node','benchmarks/history.browser.mjs',api,name],check=True,capture_output=True,text=True)
                    report['browser']=json.loads(result.stdout)
            return report
        finally:
            process.terminate()
            try:process.wait(timeout=35)
            except subprocess.TimeoutExpired:process.kill();process.wait()
            log.close()


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--binary',type=Path,default=Path('aeroboard-server/target/debug/aeroboard'))
    parser.add_argument('--baseline-binary',type=Path)
    parser.add_argument('--receipts',type=int,nargs='+',default=[10000,1000000])
    parser.add_argument('--points',type=int,default=10000)
    parser.add_argument('--history-points',type=int,default=1000000)
    parser.add_argument('--browser',action='store_true')
    args=parser.parse_args()
    for binary in filter(None,[args.baseline_binary,args.binary]):
        for receipts in args.receipts:
            for batch in [100,5000]:print(json.dumps(case(binary,receipts,batch,args.points)),flush=True)
        print(json.dumps(case(binary,0,5000,args.history_points,history=True,browser=args.browser)),flush=True)

if __name__=='__main__':main()
