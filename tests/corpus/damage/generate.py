#!/usr/bin/env python3
"""Hand-authored SQLite format-3 damage fixtures; no inspector-derived expectations."""
import json
from pathlib import Path

ROOT = Path(__file__).parent
cases = []
def put(b, at, value, size=4):
    b[at:at+size] = value.to_bytes(size, 'big')
def empty(n=2):
    b = bytearray(512*n)
    b[:16] = b'SQLite format 3\0'
    put(b, 16, 512, 2)
    b[18:24] = bytes([1,1,0,64,32,32])
    for at, val in [(28,n),(44,4),(56,1)]: put(b, at, val)
    for page in range(n):
        h = page*512 + (100 if page == 0 else 0)
        b[h] = 13
        put(b, h+5, 512, 2)
    return b
def cells():
    b = empty()
    put(b,515,2,2); put(b,517,504,2)
    b[520:524] = bytes([1,248,1,252])
    b[1016:1024] = bytes([2,1,2,8,2,2,2,8])
    return b
def eq(path, value): return {'path':path, 'equals':value}
def has(path, value): return {'path':path, 'contains':value}
def page(n, **detail): return has('/pages', {'number':n, 'detail':detail})
def case(name,b,boundary,checks,sidecars=None,fatal=False):
    (ROOT/(name+'.sqlite')).write_bytes(b)
    sc = {}
    for suffix, data in (sidecars or {}).items():
        filename = name+'.'+suffix
        (ROOT/filename).write_bytes(data); sc[suffix]=filename
    cases.append(dict(name=name, file=name+'.sqlite', boundary=boundary,
                      fatal=fatal, sidecars=sc, checks=checks))
# Geometry: dependent page interpretation must not start.
for name,b,message in [
    ('truncated-header',b'SQLite format 3\0','the database header is incomplete'),
    ('truncated-first-page',empty()[:511],'the database does not contain one complete page'),
    ('truncated-tail',empty()[:-1],'the database ends with an incomplete page'),
]:
    case(name,b,'database geometry',[
        eq('/status/diagnostic/code','fatal_geometry'),eq('/status/diagnostic/message',message),
        eq('/status/coverage/evaluated',0),eq('/status/coverage/total',None)],fatal=True)
b=empty();put(b,28,9)
case('contradictory-count',b,'database geometry',[
    eq('/status/diagnostic/code','fatal_geometry'),eq('/status/coverage/total',None),
    eq('/status/diagnostic/message','the valid database header declares 9 pages, but the file contains 2')],fatal=True)
b=empty();put(b,16,513,2)
case('impossible-page-size',b,'database geometry',[eq('/status/diagnostic/code','fatal_geometry'),eq('/status/coverage/total',None)],fatal=True)
# Independently bounded cells survive sibling damage.
for name,offset,data,index,code in [
    ('malformed-varint',522,b'\x01\xff',1,'truncated_varint'),
    ('impossible-cell-extent',1020,b'\x7f',1,'invalid_cell_extent'),
    ('out-of-range-cell',520,b'\x00\x07',0,'invalid_cell_pointer'),
    ('unsupported-serial',1023,b'\x0a',1,'invalid_record'),
    ('impossible-record-header',1022,b'\x03',1,'invalid_record'),
]:
    b=cells(); b[offset:offset+len(data)]=data
    if name=='malformed-varint': b[1023]=128
    checks=[page(1,coverage='complete'),page(2,coverage='partial'),
        has('/pages/1/detail/cells',{'identity':{'pageNumber':2,'index':index},'diagnostic':code}),
        has('/pages/1/detail/cells',{'identity':{'pageNumber':2,'index':1-index},'record':{'state':'complete'},'range':{'length':4}})]
    case(name,b,'cell pointer / extent / record; sibling preserved',checks)
b=cells();put(b,522,504,2)
case('overlapping-cells',b,'overlap component; dependent values withheld',[
    page(1,coverage='complete'),page(2,coverage='partial'),
    eq('/pages/1/detail/cells/0/diagnostic','overlapping_allocation'),
    eq('/pages/1/detail/cells/1/diagnostic','overlapping_allocation'),
    eq('/pages/1/detail/cells/0/record',None),eq('/pages/1/detail/cells/1/range',None),
    eq('/pages/1/detail/cells/0/pointer',{'pageOffset':8,'fileOffset':520,'length':2})])
b=cells();put(b,513,510,2)
case('broken-freeblock',b,'freeblock extent; cells preserved',[
    page(2,coverage='partial'),has('/pages/1/detail/diagnostics','invalid_freeblock_extent'),
    eq('/pages/1/detail/cells/0/record/state','complete'),eq('/pages/1/detail/freeblocks',[])])
# B-tree claims have stable identities; collection order is not contractual.
def tree(left=2,right=3):
    b=empty(3);b[100]=5;put(b,103,1,2);put(b,105,507,2)
    put(b,108,right);put(b,112,507,2);put(b,507,left);b[511]=1
    return b
case('out-of-range-link',tree(9),'child link; independent right branch preserved',[
    has('/relationshipClaims',{'id':'btree:page:1:cell:0','state':'unresolved','target':{'pageNumber':9},'evidence':{'range':{'pageOffset':507,'fileOffset':507,'length':4}}}),
    has('/traversals',{'kind':'btree','validatedPrefix':[{'pageNumber':1}],'stop':{'reason':'out_of_range','intendedTarget':{'pageNumber':9}}}),
    has('/traversals',{'kind':'btree','validatedPrefix':[{'pageNumber':1},{'pageNumber':3}],'stop':None}),
    has('/diagnostics',{'code':'btree_child_out_of_range','containment':'traversal_stopped'}),page(3,coverage='complete')])
case('duplicate-claims',tree(2,2),'conflicting inbound claims; no relationship authorized',[
    has('/relationshipClaims',{'id':'btree:page:1:cell:0','state':'conflicting'}),
    has('/relationshipClaims',{'id':'btree:page:1:rightmost','state':'conflicting'}),
    eq('/relationships',[]),has('/diagnostics',{'code':'btree_duplicate_parent','containment':'relationship_excluded'}),
    has('/traversals',{'validatedPrefix':[{'pageNumber':1}],'stop':{'reason':'conflicting_claim'}}),page(2,coverage='complete')])
# Freelist 2 -> 4, with leaves 3 and 5.
def freelist():
    b=empty(5)
    for start in (512,1536): b[start:start+512]=bytes(512)
    for at,val in [(32,2),(36,4),(512,4),(516,1),(520,3),(1540,1),(1544,5)]:put(b,at,val)
    return b
b=freelist();put(b,516,127)
case('impossible-freelist-count',b,'trunk count; no leaf decoding',[
    eq('/freelist/coverage/reason','invalid_structure'),eq('/freelist/coverage/remainder',None),
    has('/traversals',{'kind':'freelist','validatedPrefix':[{'pageNumber':2}],'stop':{'reason':'invalid_reference'}}),
    has('/diagnostics',{'code':'freelist_leaf_count_exceeds_capacity','containment':'traversal_stopped'}),
    eq('/freelist/declaredCount/evidence/range',{'pageOffset':36,'fileOffset':36,'length':4})])
b=freelist();put(b,1536,2)
case('freelist-cycle',b,'trunk back-link; prefix retained',[
    eq('/freelist/coverage/reason','invalid_structure'),
    has('/traversals',{'kind':'freelist','validatedPrefix':[{'pageNumber':2},{'pageNumber':4}],'stop':{'reason':'cycle','intendedTarget':{'pageNumber':2}}}),
    has('/diagnostics',{'code':'freelist_trunk_cycle','containment':'traversal_stopped'}),eq('/freelist/coverage/evaluatedPages',4)])
# Unsupported bytes are facts, not guesses about encryption/compression implementations.
for name,raw in [('encrypted-looking',bytes([0xa5])*512),('compressed',b'\x1f\x8b'+bytes([0xcc])*510),('custom-vfs',b'CUSTOM-VFS'+bytes([0x55])*502)]:
    case(name,raw,'unsupported main-file format; geometry unknown',[
        eq('/status/diagnostic/code','unsupported_format'),
        eq('/status/diagnostic/opaqueRange',{'fileOffset':'0','length':'512'}),
        eq('/status/coverage/total',None)],fatal=True)
b=empty();b[512:]=bytes([0xa5])*512
case('opaque-page',b,'unsupported page; valid geometry retained',[
    page(2,coverage='unsupported',diagnostics=[],cells=[]),
    has('/pages/1/detail/regions',{'kind':'opaque_content','range':{'pageOffset':0,'fileOffset':512,'length':512}}),page(1,coverage='complete')])
b=empty();b[20]=8
for h in [100,512]:put(b,h+5,504,2)
for at in [504,1016]:b[at:at+8]=b'RAW_HIDE'
case('reserved-content',b,'reserved extent; no extension decoding',[
    page(1,coverage='complete'),page(2,coverage='complete'),
    has('/pages/1/detail/regions',{'kind':'opaque_reserved','range':{'pageOffset':504,'fileOffset':1016,'length':8}})])
case('malformed-sidecars',cells(),'sidecar header; main file unchanged',[
    has('/sidecars',{'kind':'wal','state':'malformed','consequence':'wal_not_applied','coverage':{'reason':'validation_stop'},'diagnostics':[{'code':'wal_header_truncated','offset':'0','length':'8'}]}),
    has('/sidecars',{'kind':'journal','state':'malformed','consequence':'rollback_not_applied'}),
    has('/sidecars',{'kind':'shm','state':'malformed','consequence':'shm_non_authoritative'}),page(2,coverage='complete')],
    sidecars={suffix:b'RAW_HIDE' for suffix in ['wal','journal','shm']})
# Overflow links are interpreted independently of the record header.
def overflow(next_page):
    b=empty(3);b[512:]=bytes(1024)
    put(b,103,1,2);put(b,105,466,2);put(b,108,466,2)
    b[466:469]=bytes([0x87,0x68,1]);put(b,508,2);put(b,512,next_page)
    return b
for name,n,reason,code in [('overflow-cycle',2,'cycle','overflow_cycle'),('broken-overflow',0,'invalid_reference','overflow_chain_truncated')]:
    case(name,overflow(n),'overflow successor; validated prefix retained',[
        has('/traversals',{'kind':'overflow','validatedPrefix':[{'pageNumber':2}],'stop':{'reason':reason,'claimId':'overflow:page:2'}}),
        has('/diagnostics',{'code':code,'containment':'traversal_stopped'}),
        has('/relationshipClaims',{'id':'overflow:page:2','evidence':{'range':{'pageOffset':0,'fileOffset':512,'length':4}}})])
b=empty(3);b[512:1024]=bytes(512);put(b,52,500);b[512]=5;put(b,513,500)
case('invalid-pointer-map',b,'pointer-map entry; forward facts remain authoritative',[
    eq('/pointerMap/applicable',True),eq('/pages/1/classification/role','pointer_map'),
    eq('/pointerMap/pages/0/entries/0/state','invalid'),eq('/pointerMap/pages/0/entries/0/parentValue',500),
    eq('/pointerMap/pages/0/entries/0/evidence/range',{'pageOffset':0,'fileOffset':512,'length':5}),
    has('/diagnostics',{'code':'pointer_map_invalid_largest_root'}),page(3,coverage='complete')])
# Fixed worked WAL checksum examples, both checksum byte orders.
for order,head in [
    ('little','377f0682002de21800000200000000000000000100000002d5cb03138fa4dab80000000200000002000000010000000220637ace7cb13cdd'),
    ('big','377f0683002de218000002000000000000000001000000021604cad8bcdda493000000020000000200000001000000021efc78e3fd2bdea5')]:
    wal=bytearray.fromhex(head)+bytearray(512)
    wal.extend(bytes([0,0,0,3]))
    case('wal-tail-'+order,empty(),'WAL frame 2 truncated; first commit prefix preserved',[
        has('/sidecars',{'kind':'wal','state':'malformed','consequence':'wal_not_applied',
            'wal':{'validatedFrames':'1','lastCommitFrame':'1'},'coverage':{'reason':'validation_stop','evaluatedBytes':'572','remainingBytes':'0'},
            'diagnostics':[{'code':'wal_frame_truncated','offset':'568','length':'4'}]}),
        page(1,coverage='complete'),page(2,coverage='complete')],sidecars={'wal':wal})
journal=bytearray(1032);journal[:8]=bytes.fromhex('d9d505f920a163d7')
for at,val in [(8,1),(12,17),(16,2),(20,512),(24,512)]:put(journal,at,val)
put(journal,8,0xffffffff-1)
case('journal-record-extent',empty(),'rollback record count; no journal application',[
    has('/sidecars',{'kind':'journal','state':'malformed','journal':{'headerValid':True,'hotStatus':'unknown'},
        'diagnostics':[{'code':'journal_declared_records_truncated','offset':'8','length':'4'}]})],sidecars={'journal':journal})
# Native little-endian wal-index header with two identical, checksum-invalid copies.
shm=bytearray(32768);shm[0:4]=(3007000).to_bytes(4,'little');shm[12]=1
shm[14:16]=(512).to_bytes(2,'little');shm[16:20]=(1).to_bytes(4,'little');shm[48:96]=shm[:48]
case('shm-header-checksum',empty(),'SHM header copies; no authority over main file',[
    has('/sidecars',{'kind':'shm','consequence':'shm_non_authoritative','shm':{'copiesMatch':True,'headerChecksumsValid':False},'coverage':{'reason':'validation_stop'}})],sidecars={'shm':shm})
# A valid record seed makes successful deep reconstruction reachable by mutation.
case('valid-cells',cells(),'control: independently valid cell records',[
    page(1,coverage='complete'),page(2,coverage='complete'),eq('/pages/1/detail/cells/0/record/state','complete')])
(ROOT/'manifest.json').write_text(json.dumps(cases,indent=2)+'\n')
