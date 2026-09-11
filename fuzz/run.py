#!/usr/bin/env python3
"""Deterministic mutation targets with per-candidate process resource limits."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import random
import resource
import signal
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
TARGETS = ('database', 'traversal', 'records', 'sidecars', 'selectors', 'helper', 'http', 'lifecycle')
MAX_INPUT = 32768

def limits():
    resource.setrlimit(resource.RLIMIT_AS, (1024**3, 1024**3))
    resource.setrlimit(resource.RLIMIT_CPU, (10, 10))
    resource.setrlimit(resource.RLIMIT_FSIZE, (64*1024**2, 64*1024**2))
    resource.setrlimit(resource.RLIMIT_NOFILE, (128, 128))
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))

def mutate(seed, rng, preserve_header):
    data = bytearray(seed)
    floor = 100 if preserve_header and len(data) > 100 else 0
    for _ in range(rng.randrange(1, 9)):
        op = rng.randrange(5)
        if not data: data.append(rng.randrange(256))
        at = rng.randrange(min(floor,len(data)-1),len(data))
        if op == 0: data[at] ^= 1 << rng.randrange(8)
        elif op == 1: data[at] = rng.choice([0,1,2,5,8,10,11,13,127,128,255])
        elif op == 2:
            end = min(len(data), at+rng.randrange(1,17))
            data[at:end] = bytes([rng.choice([0,128,255])])*(end-at)
        elif op == 3 and not preserve_header: del data[at:]
        elif op == 4 and not preserve_header: data[at:at] = bytes([rng.randrange(256)])*rng.randrange(1,65)
    return bytes(data[:MAX_INPUT])

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profile', choices=['continuous','extended'], default='continuous')
    parser.add_argument('--target', choices=TARGETS)
    parser.add_argument('--seed', type=int, default=15)
    parser.add_argument('--iterations', type=int, help='mutations per target, in addition to all seeds (0..10000)')
    parser.add_argument('--replay', type=Path, help='exact retained .bin input; requires --target')
    parser.add_argument('--artifacts', type=Path, default=ROOT/'target/hostile-reproducers')
    args = parser.parse_args()
    count = args.iterations if args.iterations is not None else (16 if args.profile=='continuous' else 1000)
    if not 0 <= count <= 10000: parser.error('iterations must be 0..10000')
    if args.replay and not args.target: parser.error('--replay requires --target')
    if args.replay and args.replay.stat().st_size > MAX_INPUT: parser.error('reproducer exceeds 32768 bytes')
    subprocess.run(['cargo','build','--example','hostile_fuzz','--bin','volmap-sqlite'],cwd=ROOT,check=True)
    artifact = args.artifacts.resolve(); artifact.mkdir(parents=True,exist_ok=True)
    env = dict(os.environ, VOLMAP_HELPER_BIN=str(ROOT/'target/debug/volmap-sqlite'), RUST_BACKTRACE='1')
    files = sorted((ROOT/'tests/corpus/damage').glob('*.sqlite'))
    database = [file.read_bytes() for file in files]
    protocol = [b'',b'RAW_HIDE',b'VOLMAP-SEMANTIC-1\n',b'{"version":1,"queryMask":31,"tables":[]}',
                b'{"type":"page","pageNumber":4294967295}',b'{"type":"cell","pageNumber":0,"cellIndex":65535}',
                b'{"sessionId":"RAW_HIDE","snapshotId":"RAW_HIDE","revision":18446744073709551615,"pageNumber":0,"cellIndex":65535}',
                b'{"maxPayloadBytes":18446744073709551615,"maxOverflowPages":4294967295,"maxValues":4294967295}',
                b'9'*1024,b'['*256+b'0'+b']'*256,b'{"version":1,"version":2}',b'http://RAW_HIDE/',b'\xff'*4097,b' '*8193]
    sidecars = [file.read_bytes() for file in sorted((ROOT/'tests/corpus/damage').iterdir()) if file.suffix in ('.wal','.journal','.shm')]
    total = 0
    for target in ([args.target] if args.target else TARGETS):
        rng = random.Random(args.seed + TARGETS.index(target))
        seeds = database if target in ('database','traversal','records') else (sidecars+database+protocol if target=='sidecars' else protocol)
        inputs = [args.replay.read_bytes()] if args.replay else seeds + [mutate(rng.choice(seeds),rng,target in ('traversal','records')) for _ in range(count)]
        print(f'{target}: {len(inputs)} bounded candidates',flush=True)
        for index,data in enumerate(inputs):
            digest=hashlib.sha256(data).hexdigest()[:16]
            name=f'{target}-{args.seed}-{index}-{digest}'
            current=artifact/(name+'.bin'); current.write_bytes(data)
            meta=artifact/(name+'.json')
            command=[str(ROOT/'target/debug/examples/hostile_fuzz'),target,str(current)]
            meta.write_text(json.dumps({'target':target,'seed':args.seed,'index':index,'sha256':hashlib.sha256(data).hexdigest(),
                'command':command,'limits':{'input_bytes':MAX_INPUT,'address_space_bytes':1024**3,'cpu_seconds':10,'wall_seconds':15}},indent=2)+'\n')
            try:
                child=subprocess.Popen(command,cwd=ROOT,env=env,preexec_fn=limits,start_new_session=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
                output,_=child.communicate(timeout=15)
                success=child.returncode == 0
            except subprocess.TimeoutExpired:
                os.killpg(child.pid,signal.SIGKILL)
                output,_=child.communicate()
                success=False; output+=b'\n15-second watchdog expired\n'
            if not success:
                (artifact/(name+'.log')).write_bytes(output)
                print(output.decode(errors='replace'))
                print(f'Reproducer retained: {current}\nReplay: python3 fuzz/run.py --target {target} --replay {current}',file=sys.stderr)
                return 1
            current.unlink();meta.unlink();total+=1
    print(f'PASS: {total} candidates; seed {args.seed}; profile {args.profile}')
    return 0

if __name__=='__main__': sys.exit(main())
