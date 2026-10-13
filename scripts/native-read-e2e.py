#!/usr/bin/env python3
"""Require actual PostgreSQL migrations/seed -> API -> native CLI/MCP -> restart."""
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import urllib.request

ROOT=Path(__file__).resolve().parent.parent

def main():
    assert os.environ.get('DATABASE_URL'), 'DATABASE_URL required; this gate cannot skip storage'
    metadata=json.loads(subprocess.check_output(['cargo','metadata','--format-version','1','--no-deps'],cwd=ROOT))
    binaries=Path(metadata['target_directory'])/'debug'
    subprocess.run(['cargo','build','--locked','-p','eth-tools-dev-server','-p','eth-tools-cli'],cwd=ROOT,check=True)
    with socket.socket() as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
    base=f'http://127.0.0.1:{port}'
    env=dict(os.environ,ETH_TOOLS_LISTEN_ADDR=f'127.0.0.1:{port}',ETH_TOOLS_AUTO_SEED='1')
    process=None
    with tempfile.TemporaryFile() as log:
        def request(path,payload=None):
            req=urllib.request.Request(base+path,data=None if payload is None else json.dumps(payload).encode(),headers={'content-type':'application/json','accept':'application/json'})
            with urllib.request.urlopen(req,timeout=10) as response:return json.loads(response.read())
        def start():
            nonlocal process
            process=subprocess.Popen([str(binaries/'dev-server')],cwd=ROOT,env=env,stdout=log,stderr=subprocess.STDOUT)
            deadline=time.monotonic()+30
            while time.monotonic()<deadline:
                if process.poll() is not None:log.seek(0);raise RuntimeError(log.read().decode())
                try:return request('/api/v1/health')
                except OSError:time.sleep(.1)
            raise RuntimeError('API readiness deadline')
        def stop():
            if process is not None:
                process.terminate()
                try:process.wait(timeout=5)
                except subprocess.TimeoutExpired:process.kill();process.wait()
        try:
            health=start();assert health['status']=='ok' and health['agents_indexed']>=6
            page=request('/api/v1/agents');agent=page['data'][0];assert agent['agent_id']=='424242' and agent['agent_uri']=='https://example.invalid/final.json';reference=f"{agent['chain']}/{agent['agent_id']}"
            cli=json.loads(subprocess.check_output([str(binaries/'eth-tools'),'--api-url',base,'inspect',reference],cwd=ROOT));assert cli['data']==agent
            listed=json.loads(subprocess.check_output([str(binaries/'eth-tools'),'--api-url',base,'find',agent['agent_id']],cwd=ROOT));assert any(a['agent_id']==agent['agent_id'] for a in listed['data'])
            initialized=request('/api/mcp',{'jsonrpc':'2.0','id':1,'method':'initialize','params':{'protocolVersion':'2025-03-26','capabilities':{},'clientInfo':{'name':'native-gate','version':'1'}}});assert initialized['result']['capabilities']['tools']=={}
            tools=request('/api/mcp',{'jsonrpc':'2.0','id':2,'method':'tools/list','params':{}});assert {t['name'] for t in tools['result']['tools']}=={'health','inspect_agent','search_agents'}
            result=request('/api/mcp',{'jsonrpc':'2.0','id':3,'method':'tools/call','params':{'name':'inspect_agent','arguments':{'chain':agent['chain'],'agent_id':agent['agent_id']}}});assert not result['result']['isError'];assert json.loads(result['result']['content'][0]['text'])['data']==agent
            unknown=request('/api/mcp',{'jsonrpc':'2.0','id':4,'method':'tools/call','params':{'name':'register_agent','arguments':{}}});assert unknown['error']['code']==-32602
            stop();env['ETH_TOOLS_AUTO_SEED']='0';start();assert request('/api/v1/agents/'+reference)['data']==agent
            print('PASS: actual PostgreSQL migrations/storage -> API -> native CLI/MCP -> restart persistence')
        finally:stop()

if __name__=='__main__':main()
