#!/usr/bin/env python3
"""Black-box verification of a copied standalone executable, using frozen files."""
import argparse
import contextlib
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import selectors
import shutil
import signal
import sqlite3
import subprocess
import tempfile
import time

ROOT=Path(__file__).resolve().parent.parent

def request(port,path,body=None):
    connection=http.client.HTTPConnection('127.0.0.1',port,timeout=30)
    headers={'Host':f'127.0.0.1:{port}'}
    if body is not None: headers.update({'Origin':f'http://127.0.0.1:{port}','Content-Type':'application/json'})
    connection.request('GET' if body is None else 'POST',path,body=None if body is None else json.dumps(body),headers=headers)
    response=connection.getresponse();code=response.status;raw=response.read();connection.close()
    return code,json.loads(raw)

@contextlib.contextmanager
def server(binary,database,*args,timeout=30):
    # Runtime directory contains only the executable; PATH has no application tools.
    env=dict(os.environ,PATH=str(binary.parent/'no-tools'))
    process=subprocess.Popen([str(binary),str(database),*args],cwd=binary.parent,env=env,
        stdout=subprocess.PIPE,stderr=subprocess.PIPE,start_new_session=True)
    logs=b''; selector=selectors.DefaultSelector();selector.register(process.stderr,selectors.EVENT_READ)
    try:
        deadline=time.monotonic()+timeout
        while not re.search(rb'Page atlas: [^\n]+\n',logs):
            assert time.monotonic()<deadline,logs
            if selector.select(.2):
                block=os.read(process.stderr.fileno(),8192);assert block,logs;logs+=block
        match=re.search(rb'Page atlas: http://([^/]+)(/[^\s]+)',logs);assert match,logs
        authority,entry=(part.decode() for part in match.groups());port=int(authority.rsplit(':',1)[1])
        # The HTML carries the opaque snapshot id; no filesystem path is needed by the browser.
        connection=http.client.HTTPConnection('127.0.0.1',port,timeout=30)
        connection.request('GET',entry);response=connection.getresponse();html=response.read();connection.close()
        snapshot=re.search(rb'name="inspection-snapshot" content="([^"]+)"',html)[1].decode()
        base='/api/snapshots/'+snapshot
        while True:
            code,status=request(port,base);assert code==200
            if status['state']!='scanning':break
            assert time.monotonic()<deadline,status
            time.sleep(.02)
        yield port,base,status,entry,logs
    finally:
        if process.poll() is None: os.killpg(process.pid,signal.SIGINT)
        try: out,err=process.communicate(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid,signal.SIGKILL);process.communicate();raise AssertionError('artifact failed to stop')
        selector.close()
        assert str(database.parent).encode() not in logs+out+err
        for secret in [b'PRIVATE_ROW',b'OTHER_ROW',b'RAW_HIDE']:assert secret not in logs+out+err

def fixture(path,one=False):
    with sqlite3.connect(path) as db:
        db.executescript("PRAGMA page_size=512; CREATE TABLE items(value TEXT,payload BLOB); INSERT INTO items VALUES('PRIVATE_ROW',x'5241575f48494445');")
        if not one: db.execute("INSERT INTO items VALUES('OTHER_ROW',x'5241575f48494445')")

def physical(graph):
    return {key:graph[key] for key in ['pages','relationshipClaims','relationships','traversals','diagnostics','schema','freelist','pointerMap','coverage','topologyCoverage']}

def smoke(binary,chromium,extended=False):
    with tempfile.TemporaryDirectory(prefix='volmap-package-') as directory:
        root=Path(directory);runtime=root/'runtime';runtime.mkdir();copied=runtime/'volmap-sqlite';shutil.copy2(binary,copied)
        database=root/'frozen.sqlite';fixture(database)
        initial=hashlib.sha256(database.read_bytes()).hexdigest()
        with server(copied,database,'--listen','127.0.0.1:0','--semantic-metadata') as (port,base,status,entry,logs):
            assert status['state']=='published';assert b'WARNING' not in logs
            graph=request(port,base+'/revisions/1')[1]
            assert graph['semanticMetadata']['state']=='available'
            baseline=physical(graph)
            assert any(obj['name']=='items' and obj['pages'] for obj in graph['schema']['objects'])
            assert graph['coverage']['reason']=='complete'
            for secret in ['PRIVATE_ROW','OTHER_ROW','RAW_HIDE',directory]:assert secret not in json.dumps(graph)
            subprocess.run(['node',str(ROOT/'release/browser.mjs'),f'http://127.0.0.1:{port}{entry}',str(chromium)],check=True,timeout=45)
            assert request(port,base+'/revisions/1')[1]==graph
            # Source modification invalidates every historical revision in the built artifact.
            with database.open('r+b') as file:file.seek(60);file.write(b'\x00\x00\x00\x01')
            assert request(port,base+'/revisions/1')[0]==409
        # Restore fixture; no inspector operation itself modified any input bytes.
        with database.open('r+b') as file:file.seek(60);file.write(bytes(4))
        assert hashlib.sha256(database.read_bytes()).hexdigest()==initial
        for suffix in ['wal','journal','shm']:(root/('frozen.sqlite-'+suffix)).write_bytes(b'RAW_HIDE')
        with server(copied,database,'--listen','127.0.0.1:0') as (port,base,status,entry,logs):
            graph=request(port,base+'/revisions/1')[1];assert physical(graph)==baseline
            assert len(graph['sidecars'])==3
            assert all(sc['state']=='malformed' for sc in graph['sidecars'])
        with server(copied,database,'--listen','127.0.0.1:0','--max-processed-cells','0') as (port,base,status,entry,logs):
            assert status['state']=='stopped';assert status['coverage']['reason']=='cell_budget'
            assert status['coverage']['evaluated']==0
        corpus=ROOT/'tests/corpus/damage';cases=json.loads((corpus/'manifest.json').read_text())
        for case in cases:
            damaged=root/'damaged.sqlite';shutil.copyfile(corpus/case['file'],damaged)
            with server(copied,damaged,'--listen','127.0.0.1:0') as (port,base,status,entry,logs):
                assert status['state']==('fatal' if case['fatal'] else 'published'),case['name']
                if not case['fatal']:
                    graph=request(port,base+'/revisions/1')[1]
                    assert graph['coverage']['reason']=='complete'
        terminal=root/'terminal.sqlite';fixture(terminal,one=True)
        subprocess.run([shutil.which('python3'),str(ROOT/'tests/support/terminal_pty.py'),str(copied),str(terminal)],check=True,timeout=45,env=dict(os.environ,PATH=str(runtime/'no-tools')))
        env=dict(os.environ,VOLMAP_TEST_CHROMIUM=str(chromium))
        subprocess.run(['python3',str(ROOT/'tests/support/web_security.py'),str(copied)],check=True,timeout=180,env=env)
        if extended:
            huge=root/'two-gib.sqlite';pages=32768;page_size=65536
            header=bytearray(100);header[:16]=b'SQLite format 3\0';header[16:18]=b'\0\1';header[18:24]=bytes([1,1,0,64,32,32])
            for at,value in [(28,pages),(44,4),(56,1)]:header[at:at+4]=value.to_bytes(4,'big')
            with huge.open('wb') as file:file.write(header);file.write(bytes([13])+bytes(7));file.truncate(pages*page_size)
            with server(copied,huge,'--listen','127.0.0.1:0','--page-cache-bytes','1048576','--max-resident-bytes','67108864',timeout=600) as (port,base,status,entry,logs):
                assert status['coverage']['reason']=='complete',status
                assert status['coverage']['evaluated']==pages
                metadata=request(port,base+'/revisions/1/metadata')[1]
                assert metadata['summary']['topologyCoverage']['reason']=='complete'
                assert metadata['schema']['state']=='complete'
                for page in [1,pages]:assert request(port,base+f'/revisions/1/pages/{page}/1')[1]['pages'][0]['number']==page
        print('Artifact smoke passed: browser, terminal, metadata helper, disclosure, sidecars, invalidation, budgets, damage corpus and network isolation')

if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('binary',type=Path);parser.add_argument('--chromium',type=Path,required=True);parser.add_argument('--extended',action='store_true')
    args=parser.parse_args();smoke(args.binary.resolve(),args.chromium.resolve(),args.extended)
