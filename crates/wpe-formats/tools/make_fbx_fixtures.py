"""Generate owned FBX7 ASCII/binary controls. No original file is read."""
from pathlib import Path
import hashlib,json,struct,zlib,copy
ROOT=Path(__file__).resolve().parents[1]/'tests/fixtures'
ROOT.mkdir(parents=True,exist_ok=True)
class Array:
 def __init__(self,kind,values):self.kind,self.values=kind,values
def node(name,props=(),children=()):return name,list(props),list(children)
def prop(name,kind,*values):return node('P',(name,kind,'','A',*values))
def properties(*values):return node('Properties70',children=values)
identity=[1.,0.,0.,0.,0.,1.,0.,0.,0.,0.,1.,0.,0.,0.,0.,1.]
ticks=46186158000
kinds=[('Model','FbxNode'),('Geometry','FbxMesh'),('Deformer','FbxSkin'),('AnimationStack','FbxAnimStack'),('AnimationLayer','FbxAnimLayer'),('AnimationCurveNode','FbxAnimCurveNode'),('AnimationCurve','FbxAnimCurve')]
header=node('FBXHeaderExtension',children=[node('FBXHeaderVersion',[1003]),node('FBXVersion',[7400]),node('Creator',['WallpaperMac owned fixture generator']),node('CreationTimeStamp',children=[node(k,[v])for k,v in [('Version',1000),('Year',2026),('Month',10),('Day',28),('Hour',0),('Minute',0),('Second',0),('Millisecond',0)]])])
settings=node('GlobalSettings',children=[properties(prop('UpAxis','int',1),prop('UpAxisSign','int',1),prop('FrontAxis','int',2),prop('FrontAxisSign','int',1),prop('CoordAxis','int',0),prop('CoordAxisSign','int',1),prop('UnitScaleFactor','double',1.),prop('TimeMode','enum',14),prop('CustomFrameRate','double',2.))])
definitions=node('Definitions',children=[node('Version',[100]),node('Count',[11]),*[node('ObjectType',[kind],[node('Count',[1]),node('PropertyTemplate',[template],[properties()])])for kind,template in kinds]])
geometry=node('Geometry',[10,'Geometry::triangle','Mesh'],[
 node('Vertices',[Array('d',[-2.,-2.,0.,2.,-2.,0.,0.,2.,0.])]),node('PolygonVertexIndex',[Array('i',[0,1,-3])]),
 node('LayerElementNormal',[0],[node('MappingInformationType',['ByPolygonVertex']),node('ReferenceInformationType',['Direct']),node('Normals',[Array('d',[0.,0.,1.]*3)])]),
 node('LayerElementUV',[0],[node('MappingInformationType',['ByPolygonVertex']),node('ReferenceInformationType',['Direct']),node('UV',[Array('d',[0.,0.,1.,0.,0.5,1.])])])])
objects=node('Objects',children=[geometry,
 node('Model',[100,'Model::mesh','Mesh'],[properties(prop('Lcl Translation','Lcl Translation',0.,0.,0.),prop('Lcl Rotation','Lcl Rotation',0.,0.,0.),prop('Lcl Scaling','Lcl Scaling',1.,1.,1.))]),
 node('Model',[200,'Model::owned-root','LimbNode'],[properties(prop('Lcl Translation','Lcl Translation',0.,0.,0.),prop('Lcl Rotation','Lcl Rotation',0.,0.,0.),prop('Lcl Scaling','Lcl Scaling',1.,1.,1.))]),
 node('Deformer',[300,'Deformer::skin','Skin']),node('Deformer',[400,'SubDeformer::cluster','Cluster'],[node('Indexes',[Array('i',[0,1,2])]),node('Weights',[Array('d',[1.,1.,1.])]),node('Transform',[Array('d',identity)]),node('TransformLink',[Array('d',identity)]),node('Mode',['Normalize'])]),
 node('AnimationStack',[500,'AnimStack::move',''],[properties(prop('LocalStart','KTime',0),prop('LocalStop','KTime',ticks))]),
 node('AnimationLayer',[520,'AnimLayer::base',''],[properties(prop('Weight','double',100.))]),
 node('AnimationCurveNode',[510,'AnimCurveNode::T',''],[properties(prop('d|X','Number',0.),prop('d|Y','Number',0.),prop('d|Z','Number',0.))]),
 node('AnimationCurve',[530,'AnimCurve::X',''],[node('Default',[0.]),node('KeyVer',[4008]),node('KeyTime',[Array('l',[0,ticks])]),node('KeyValueFloat',[Array('f',[0.,2.])]),node('KeyAttrFlags',[Array('i',[4])]),node('KeyAttrDataFloat',[Array('f',[0.,0.,0.,0.])]),node('KeyAttrRefCount',[Array('i',[2])])])])
connections=node('Connections',children=[node('C',row)for row in [('OO',10,100),('OO',100,0),('OO',200,400),('OO',400,300),('OO',300,10),('OO',520,500),('OO',510,520),('OP',510,200,'Lcl Translation'),('OP',530,510,'d|X')]])
tree=[header,settings,definitions,objects,connections]
def ascii_value(value):
 if isinstance(value,str):return json.dumps(value)
 return str(value)
def ascii_node(value,indent=0):
 name,props,children=value
 if len(props)==1 and isinstance(props[0],Array):
  a=props[0];props=['*'+str(len(a.values))];children=[node('a',a.values)]
  prefix='\t'*indent+name+': '+props[0]
 else:prefix='\t'*indent+name+': '+', '.join(map(ascii_value,props))
 if children:return prefix+' {\n'+''.join(ascii_node(child,indent+1)for child in children)+'\t'*indent+'}\n'
 return prefix+'\n'
ascii_data=('; Owned FBX 7.4 fixture\n'+''.join(map(ascii_node,tree))).encode()
def binary_prop(value):
 if isinstance(value,Array):
  raw=struct.pack('<'+{'l':'q'}.get(value.kind,value.kind)*len(value.values),*value.values);compressed=zlib.compress(raw)
  return value.kind.encode()+struct.pack('<III',len(value.values),1,len(compressed))+compressed
 if isinstance(value,str):
  raw=value.encode();return b'S'+struct.pack('<I',len(raw))+raw
 if isinstance(value,int):return b'L'+struct.pack('<q',value)
 return b'D'+struct.pack('<d',value)
def binary_node(value,start,version=7400):
 name,props,children=value;raw=b''.join(map(binary_prop,props));label=name.encode();data=raw
 header=25 if version>=7500 else 13
 child_start=start+header+len(label)+len(raw)
 for child in children:
  block=binary_node(child,child_start,version);data+=block;child_start+=len(block)
 if children:data+=bytes(header)
 end=start+header+len(label)+len(data)
 return struct.pack('<QQQB'if version>=7500 else'<IIIB',end,len(props),len(raw),len(label))+label+data
def binary_file(version):
 values=copy.deepcopy(tree)
 values[0][2][1][1][0]=version
 data=b'Kaydara FBX Binary  \x00\x1a\x00'+struct.pack('<I',version)
 for value in values:data+=binary_node(value,len(data),version)
 data+=bytes(25 if version>=7500 else 13)+bytes.fromhex('f0b1a203d4c5d667b879fa8b1cfd2e7f')
 data+=bytes((-len(data))%16)+bytes(4)+struct.pack('<I',version)+bytes(120)+bytes.fromhex('f85a8c6adef5d97eece90ce3758f290b')
 return data
binary_data=binary_file(7400)
wide_data=binary_file(7500)
files=[]
for name,data in [('owned-skinned-animation-ascii.fbx',ascii_data),('owned-skinned-animation-binary.fbx',binary_data),('owned-skinned-animation-wide-binary.fbx',wide_data)]:
 (ROOT/name).write_bytes(data);files.append(dict(path=name,bytes=len(data),sha256=hashlib.sha256(data).hexdigest()))
(ROOT/'fbx-manifest.json').write_text(json.dumps(dict(classification='entirely-owned-generated-FBX-controls',generator='tools/make_fbx_fixtures.py',original_assets_read=False,files=files),indent=2)+'\n')
print(json.dumps(files))
