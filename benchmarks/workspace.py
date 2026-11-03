"""Small repeatable SDK/delivery/analysis fixture; start a server first.

Use --sdk-source PATH to compare an older SDK checkout against the same server.
This is a workstation diagnostic, not a throughput or memory SLA.
"""
import argparse
import json
import math
from pathlib import Path
import resource
import statistics
import sys
import tempfile
import time
import uuid

import requests


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("--sdk-source",type=Path)
    parser.add_argument("--api",default="http://127.0.0.1:3000")
    parser.add_argument("--flight",default="grpc://127.0.0.1:50051")
    parser.add_argument("--rows",type=int,default=5000)
    parser.add_argument("--persistent",action="store_true")
    args=parser.parse_args()
    if args.sdk_source: sys.path.insert(0,str(args.sdk_source))
    from aeroboard_client import AeroboardClient
    def request(method,path,**kwargs):
        result=requests.request(method,args.api+"/api"+path,timeout=30,**kwargs)
        result.raise_for_status();return result.json()
    with tempfile.TemporaryDirectory(prefix="aeroboard-benchmark-") as spool:
        client=AeroboardClient("benchmark_"+uuid.uuid4().hex,url=args.flight,api_base_url=args.api,
            batch_size=100,flush_timeout=30,**({"spool_dir":spool} if args.persistent else {}))
        name=client.run_id
        client.log({"loss":1.0,"accuracy":0.0},step=-1);client.flush()
        name=client.run_id
        uid=request("GET","/run-records/"+name)["id"]
        request("POST","/metric-definitions",json={"parent":uid,"name":"loss","goal":"minimize","target":0.1})
        samples=[];started=time.perf_counter()
        for step in range(args.rows):
            loss=math.exp(-step/max(1,args.rows/5))
            tick=time.perf_counter_ns();client.log({"loss":loss,"accuracy":1-loss},step=step)
            samples.append((time.perf_counter_ns()-tick)/1000)
        logging_ms=(time.perf_counter()-started)*1000
        local_ms=None;spool_bytes=None
        if args.persistent:
            tick=time.perf_counter();assert client.flush(local_only=True)
            local_ms=(time.perf_counter()-tick)*1000;spool_bytes=client.delivery_status()["bytes"]
        tick=time.perf_counter();client.flush();flush_ms=(time.perf_counter()-tick)*1000
        delivery_ms=(time.perf_counter()-started)*1000
        tick=time.perf_counter();score=request("GET","/scorecards",params={"run_id":name});cold_ms=(time.perf_counter()-tick)*1000
        assert next(s for s in score["rows"] if s["name"]=="loss")["finite_count"]==args.rows+1
        warm=[]
        for _ in range(10):
            tick=time.perf_counter();request("GET","/scorecards",params={"run_id":name});warm.append((time.perf_counter()-tick)*1000)
        tick=time.perf_counter();request("POST","/comparison/aggregate",json={"run_ids":[name],"metric":"loss","include_unfinished":True});aggregation_ms=(time.perf_counter()-tick)*1000
        client.close()
        result={"source":str(args.sdk_source or "working tree"),"persistent":args.persistent,"observations":2*(args.rows+1),
            "log_total_ms":logging_ms,"log_median_us":statistics.median(samples),"log_p95_us":sorted(samples)[int(len(samples)*0.95)],
            "flush_ms":flush_ms,"local_flush_ms":local_ms,"spool_bytes_at_local_barrier":spool_bytes,
            "delivery_ms":delivery_ms,"score_cold_ms":cold_ms,"score_warm_median_ms":statistics.median(warm),"aggregation_ms":aggregation_ms,
            "process_peak_rss_mib":resource.getrusage(resource.RUSAGE_SELF).ru_maxrss/(1024**2 if sys.platform=="darwin" else 1024)}
        print(json.dumps(result,sort_keys=True))


if __name__=="__main__": main()
