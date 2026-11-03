"""Creates entirely owned tiny MDLV/MDLS/MDLA fixtures; reads no target asset."""
from pathlib import Path
import hashlib,json,struct
root=Path(__file__).resolve().parents[1]/'tests/fixtures'
root.mkdir(parents=True,exist_ok=True)
u32=lambda n:struct.pack('<I',n)
floats=lambda values:struct.pack('<'+'f'*len(values),*values)
def section(out,magic,body):
    out+=magic.encode()+b'\0'
    out+=u32(len(out)+4+len(body))+body
def mesh(skinned):
    vf=0x180000f if skinned else 9
    out=bytearray(b'MDLV0023\0'+u32(vf)+u32(0)+u32(1)+u32(0))
    out+=floats([-5,-5,0,5,5,0])+u32(vf)
    vertices=bytearray()
    for position,uv in [((-5,-5,0),(0,1)),((5,-5,0),(1,1)),((0,5,0),(0.5,0))]:
        vertices+=floats(position)
        if skinned:
            vertices+=floats([0,0,1,1,0,0,1])+struct.pack('<4I',0,0,0,0)+floats([1,0,0,0])
        vertices+=floats(uv)
    out+=u32(len(vertices))+vertices+u32(6)+struct.pack('<3H',0,1,2)+b'\0\0'+u32(0)
    if skinned:
        identity=[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1]
        body=u32(1)+b'owned-root\0'+u32(0)+struct.pack('<i',-1)+u32(64)+floats(identity)+b'{}\0'
        body+=struct.pack('<HBIHHBBB',0,0,0,0,0,0,0,0)
        section(out,'MDLS0004',body)
        body=bytearray(u32(1)+struct.pack('<Q',7)+b'owned-motion\0loop\0'+floats([2])+u32(2)+u32(0)+u32(1))
        poses=b''.join(floats([x,0,0,0,0,0,1,1,1])for x in [0,1,2])
        body+=u32(0)+u32(len(poses))+poses
        body+=u32(0)+b'\1'+u32(0)+u32(12)+floats([1,0.5,1])+b'\0'+u32(0)*6
        body+=b'\1'+u32(0)+u32(12)+floats([0,1,0])
        body+=u32(1)+floats([0.25])+b'{"name":"owned-quarter-second"}\0'
        section(out,'MDLA0006',body)
    out+=b'\0'
    return out
files=[]
for name,skinned in [('editable-triangle.mdl',False),('editable-skinned-triangle.mdl',True)]:
    data=mesh(skinned);(root/name).write_bytes(data)
    files.append(dict(path=name,bytes=len(data),sha256=hashlib.sha256(data).hexdigest()))
(root/'manifest.json').write_text(json.dumps({'classification':'entirely-owned-generated-fixtures','original_assets_read':False,'generator':str(Path(__file__).name),'files':files},indent=2)+'\n')
print(json.dumps(files))
