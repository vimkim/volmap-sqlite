#!/usr/bin/env python3
"""Verify release behavior and byte reproducibility from immutable clean Git trees."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT=Path(__file__).resolve().parent.parent

def command(args,cwd,env=None):
    subprocess.run(args,cwd=cwd,env=env,check=True)

def verify(tree,chromium,output,extended):
    tree=subprocess.check_output(['git','rev-parse',tree+'^{tree}'],cwd=ROOT,text=True).strip()
    with tempfile.TemporaryDirectory(prefix='volmap-clean-build-') as temp:
        workspace=Path(temp);reports=[]
        for name in ['first','second']:
            source=workspace/name;source.mkdir()
            archive=workspace/(name+'.tar')
            with archive.open('wb') as file:subprocess.run(['git','archive',tree],cwd=ROOT,stdout=file,check=True)
            command(['tar','xf',str(archive)],source)
            destination=workspace/(name+'-artifact')
            command(['python3','release/build.py','--output',str(destination)],source)
            reports.append(json.loads((destination/'build-info.json').read_text()))
            if name=='first':
                env=dict(os.environ,VOLMAP_TEST_CHROMIUM=str(chromium))
                command(['cargo','fmt','--','--check'],source,env)
                command(['cargo','clippy','--locked','--all-targets','--all-features','--','-D','warnings'],source,env)
                command(['cargo','test','--locked','--all-targets','--all-features'],source,env)
                command(['npm','test'],source/'frontend',env)
                command(['python3','release/smoke.py',str(destination/'volmap-sqlite'),'--chromium',str(chromium)]+(['--extended'] if extended else []),source,env)
        assert reports[0]['sha256']==reports[1]['sha256'], 'Independent clean builds differ'
        assert reports[0]==reports[1], 'Build manifests differ'
        output.mkdir(parents=True,exist_ok=True)
        for filename in ['volmap-sqlite','build-info.json','SHA256SUMS']:shutil.copy2(workspace/'first-artifact'/filename,output/filename)
        report={'sourceTree':tree,'profile':'extended' if extended else 'continuous','cleanBuilds':2,'byteIdentical':True,
            'artifactSha256':reports[0]['sha256'],'version':reports[0]['version'],
            'browserVersion':subprocess.check_output([str(chromium),'--version'],text=True).strip()}
        (output/'verification.json').write_text(json.dumps(report,indent=2)+'\n')
        print(json.dumps(report,indent=2))

if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tree',default='HEAD',help='immutable Git commit or tree (default HEAD)')
    parser.add_argument('--chromium',type=Path,required=True)
    parser.add_argument('--output',type=Path,default=ROOT/'target/verified-v1')
    parser.add_argument('--extended',action='store_true')
    args=parser.parse_args();verify(args.tree,args.chromium.resolve(),args.output.resolve(),args.extended)
