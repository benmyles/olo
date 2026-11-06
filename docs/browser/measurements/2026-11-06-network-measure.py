#!/usr/bin/python3
"""Run frozen before/after page_bench binaries on CPU 0. No concurrent builds
or owned GUI clients. Compile both against the same new harness: before uses
6251094a's page.rs, after uses the current page.rs. This isolates the pipeline
change (ordered resources and retained compiled rules), not HTTP throughput.
Args: /var/tmp/OWNED (contains page-before/page-after), OUTPUT.json
"""
import hashlib,json,platform,subprocess,sys,time
from pathlib import Path
root=Path(sys.argv[1]);rows=[]
for repeat in range(3):
    order=['before','after'] if repeat%2==0 else ['after','before']
    for revision in order:
        binary=root/f'page-{revision}'
        result=subprocess.run(['/usr/bin/time','-f','{"peak_rss_kib":%M,"user_seconds":%U,"system_seconds":%S,"elapsed_seconds":%e}','taskset','-c','0',str(binary),'150'],capture_output=True,text=True,check=True)
        data=json.loads(result.stdout);assert data['features']==[],data['features']
        rows.append({'repeat':repeat,'revision':revision,'binary_sha256':hashlib.file_digest(binary.open('rb'),'sha256').hexdigest(),'result':data,'cost':json.loads(result.stderr)})
reference=rows[0]['result']['rows']
for row in rows:
    for actual,expected in zip(row['result']['rows'],reference):
        for key in ['elements','commands','geometry_sha256','source_sha256']:assert actual[key]==expected[key],(key,actual,expected)
Path(sys.argv[2]).write_text(json.dumps({'scope':__doc__,'machine':platform.platform(),'captured_at_unix':time.time(),'rows':rows},indent=2)+'\n')
print(json.dumps(rows,indent=2))
