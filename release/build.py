#!/usr/bin/env python3
"""Build the pinned Linux release, checking embedded asset freshness first."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import shlex
import subprocess

ROOT = Path(__file__).resolve().parent.parent

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def run(command, **options):
    return subprocess.run(command, check=True, **options)

def build(output):
    pins = json.loads((ROOT/'release/toolchain.json').read_text())
    commands = {'rustc':['rustc','--version'], 'node':['node','--version'], 'npm':['npm','--version'],
                'cc':['/usr/bin/gcc','--version'], 'ld':['/usr/bin/ld','--version'],
                'ar':['/usr/bin/ar','--version'], 'libc':['/usr/bin/ldd','--version']}
    tools = {}
    for name,command in commands.items():
        actual = run(command,cwd=ROOT,capture_output=True,text=True).stdout.splitlines()[0]
        if actual != pins[name]: raise SystemExit(f'{name}: expected {pins[name]!r}, found {actual!r}')
        executable = (Path(run(['rustup','which','rustc'],cwd=ROOT,capture_output=True,text=True).stdout.strip())
                      if name == 'rustc' else Path(shutil.which(command[0])).resolve())
        tools[name] = {'version':actual,'sha256':digest(executable)}
    env = dict(os.environ)
    cargo_home = Path(env.get('CARGO_HOME',Path.home()/'.cargo')).resolve()
    node_bin = Path(shutil.which('node')).resolve().parent
    env['PATH'] = os.pathsep.join([str(node_bin),str(cargo_home/'bin'),'/usr/bin','/bin'])
    env.update({'LC_ALL':'C','TZ':'UTC','SOURCE_DATE_EPOCH':'0','CARGO_INCREMENTAL':'0',
                'CC':'/usr/bin/gcc','AR':'/usr/bin/ar','CARGO_TARGET_DIR':str(output/'build')})
    # Do not inherit local build flags or wrappers into a reproducible release.
    for name in ['RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS','RUSTC_WRAPPER','RUSTC_WORKSPACE_WRAPPER','CFLAGS','CXXFLAGS']:
        env.pop(name,None)
    env['CARGO_ENCODED_RUSTFLAGS'] = '\x1f'.join([
        f'--remap-path-prefix={ROOT}=/volmap',f'--remap-path-prefix={cargo_home}=/cargo',
        '-C','linker=/usr/bin/gcc','-C','link-arg=-B/usr/bin/','-C','link-arg=-Wl,--build-id=none'])
    env['CFLAGS'] = shlex.join(['-B/usr/bin/',f'-ffile-prefix-map={ROOT}=/volmap',f'-ffile-prefix-map={cargo_home}=/cargo'])
    assets = ROOT/'frontend/dist'
    expected = {str(p.relative_to(assets)):digest(p) for p in assets.rglob('*') if p.is_file()}
    run(['npm','ci','--no-audit','--no-fund'],cwd=ROOT/'frontend',env=env)
    run(['npm','run','build'],cwd=ROOT/'frontend',env=env)
    actual = {str(p.relative_to(assets)):digest(p) for p in assets.rglob('*') if p.is_file()}
    if actual != expected: raise SystemExit('Embedded assets are stale: rebuild and commit frontend/dist before releasing')
    run(['cargo','build','--release','--locked','--target',pins['target'],'--bin','volmap-sqlite'],cwd=ROOT,env=env)
    binary = output/'volmap-sqlite'
    shutil.copy2(output/'build'/pins['target']/'release/volmap-sqlite',binary)
    version = run([str(binary),'--version'],capture_output=True,text=True).stdout.strip()
    if str(ROOT) in version or str(cargo_home) in version: raise SystemExit('Build identity exposes a local path')
    # Dynamic dependencies may be operating-system libraries, never SQLite or a JS runtime.
    dependencies = run(['/usr/bin/ldd',str(binary)],capture_output=True,text=True).stdout
    for forbidden in ['libsqlite','libnode','libv8']:
        if forbidden in dependencies: raise SystemExit('Unexpected application runtime dependency')
    manifest = {'version':version,'sha256':digest(binary),'target':pins['target'],'tools':tools,'assets':actual}
    (output/'build-info.json').write_text(json.dumps(manifest,indent=2,sort_keys=True)+'\n')
    (output/'SHA256SUMS').write_text(manifest['sha256']+'  volmap-sqlite\n')
    print('Release artifact:',binary)
    return manifest

if __name__ == '__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output',type=Path,default=ROOT/'target/v1-release')
    args=parser.parse_args()
    output=args.output.resolve();output.mkdir(parents=True,exist_ok=True)
    build(output)
