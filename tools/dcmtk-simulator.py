#!/usr/bin/env python3
"""Local DCMTK multi-device sender UI. Run: python3 tools/dcmtk-simulator.py"""
import json, os, shutil, subprocess, tempfile, threading, uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parent
PORT = int(os.getenv('SIMULATOR_PORT', '8787'))
jobs = {}

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_): pass
    def _json(self, value, code=200):
        data = json.dumps(value).encode(); self.send_response(code)
        self.send_header('Content-Type','application/json'); self.send_header('Content-Length',str(len(data))); self.end_headers(); self.wfile.write(data)
    def do_GET(self):
        if self.path == '/':
            data=(ROOT/'dcmtk-simulator.html').read_bytes(); self.send_response(200); self.send_header('Content-Type','text/html'); self.send_header('Content-Length',str(len(data))); self.end_headers(); self.wfile.write(data)
        elif self.path.startswith('/api/jobs/'):
            self._json(jobs.get(self.path.rsplit('/',1)[-1], {'error':'not found'}))
        else: self._json({'error':'not found'},404)
    def do_POST(self):
        if self.path != '/api/send': return self._json({'error':'not found'},404)
        n=int(self.headers.get('Content-Length','0')); payload=json.loads(self.rfile.read(n))
        job_id=str(uuid.uuid4()); jobs[job_id]={'state':'queued','devices':payload.get('devices',[])}
        threading.Thread(target=run_job,args=(job_id,payload),daemon=True).start(); self._json({'job_id':job_id})

def run_job(job_id, payload):
    jobs[job_id]['state']='running'; jobs[job_id]['results']=[]
    root=Path(tempfile.mkdtemp(prefix='dcmtk-sim-'))
    try:
        for device in payload.get('devices',[]):
            files=[]
            for item in device.get('files',[]):
                p=root/item['name']; p.parent.mkdir(parents=True,exist_ok=True); p.write_bytes(__import__('base64').b64decode(item['data'])); files.append(str(p))
            cmd=[shutil.which('storescu') or 'storescu','-v','-aet',device.get('calling_ae','SIM_SCU'),'-aec',device.get('called_ae','REMOTE_PACS'),device.get('host','127.0.0.1'),str(device.get('port',11112)),'+sd','+r']+files
            proc=subprocess.run(cmd,capture_output=True,text=True); jobs[job_id]['results'].append({'name':device.get('name','设备'),'sent':len(files),'ok':proc.returncode==0,'output':(proc.stderr or proc.stdout)[-2000:]})
        jobs[job_id]['state']='succeeded' if all(x['ok'] for x in jobs[job_id]['results']) else 'failed'
    except Exception as e: jobs[job_id].update(state='failed',error=str(e))
    finally: shutil.rmtree(root,ignore_errors=True)

if __name__ == '__main__':
    print(f'DICOM device simulator: http://127.0.0.1:{PORT}')
    ThreadingHTTPServer(('127.0.0.1',PORT),Handler).serve_forever()
