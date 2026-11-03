// Independently implemented SceneScript value classes. Methods return new values
// except documented matrix translation setters. SceneScript math angles are degrees;
// serialized scene angles are converted separately by the host.
(function(G){'use strict';
const D=Math.PI/180,R=180/Math.PI,axes=['x','y','z','w'];
const isVector=v=>v!=null&&[2,3,4].includes(v._n)&&typeof v._array==='function';
const component=(v,i)=>typeof v==='number'?v:v[axes[i]];
const clamp=(v,a,b)=>Math.min(Math.max(v,a),b);
const smooth=(a,b,v)=>{let t=clamp((v-a)/(b-a),0,1);return t*t*(3-2*t)};
class Vector {
 constructor(n,args){Object.defineProperty(this,'_n',{value:n});let first=args[0];
  if(typeof first==='string'){let values=first.split(' ');for(let i=0;i<n;i++)this[axes[i]]=parseFloat(values[i]);}
  else if(isVector(first)&&(first._n!==4||n===4)){for(let i=0;i<n;i++)this[axes[i]]=first[axes[i]]??0;}
  else{this.x=first===undefined?0:first;if(n>1)this.y=typeof args[1]==='number'?args[1]:this.x;if(n>2)this.z=typeof args[2]==='number'?args[2]:typeof args[1]==='number'?0:this.x;if(n>3)this.w=typeof args[3]==='number'?args[3]:typeof args[2]==='number'?args[2]:typeof args[1]==='number'?0:this.x;}
 }
 _array(){return axes.slice(0,this._n).map(k=>this[k])}
 _map(f){return new this.constructor(...this._array().map(f))}
 _zip(v,f){return this._map((x,i)=>f(x,component(v,i),i))}
 lengthSqr(){return this.dot(this)} length(){return Math.sqrt(this.lengthSqr())}
 distanceSqr(v){return this.subtract(v).lengthSqr()} distance(v){return Math.sqrt(this.distanceSqr(v))}
 normalize(){return this.divide(this.length())} copy(){return this._map(x=>x)}
 equals(v){return isVector(v)&&v._n===this._n&&this._array().every((x,i)=>Math.abs(x-component(v,i))<1e-5)}
 isFinite(){return this._array().every(Number.isFinite)} negate(){return this._map(x=>-x)}
 add(v){return this._zip(v,(a,b)=>a+b)} subtract(v){return this._zip(v,(a,b)=>a-b)}
 multiply(v){return this._zip(v,(a,b)=>a*b)} divide(v){return this._zip(v,(a,b)=>a/b)}
 dot(v){return this._array().reduce((s,x,i)=>s+x*component(v,i),0)}
 reflect(n){return this.subtract(n.multiply(2*this.dot(n)))}
 project(v){let q=v.lengthSqr();return q===0?new this.constructor(0):v.multiply(this.dot(v)/q)}
 mix(v,a){return this._map((x,i)=>x+(component(v,i)-x)*component(a,i))}
 min(v){return this._zip(v,(a,b)=>Math.min(a,b))} max(v){return this._zip(v,(a,b)=>Math.max(a,b))}
 clamp(a,b){return this._map((x,i)=>clamp(x,component(a,i),component(b,i)))}
 abs(){return this._map(Math.abs)} sign(){return this._map(Math.sign)} round(){return this._map(Math.round)}
 floor(){return this._map(Math.floor)} ceil(){return this._map(Math.ceil)} fract(){return this._map(x=>x-Math.floor(x))}
 mod(v){return this._zip(v,(x,y)=>x-y*Math.floor(x/y))}
 step(edge){return this._zip(edge,(x,e)=>x<e?0:1)}
 smoothStep(a,b){return this._map((x,i)=>smooth(component(a,i),component(b,i),x))}
 toString(){return this._array().join(' ')}toConfigString(){return this.toString()}
}
class Vec2 extends Vector {
 constructor(...a){super(2,a)} perpendicular(){return new Vec2(this.y,-this.x)}
 angle(){return Math.atan2(this.y,this.x)*R}
 angleBetween(v){return Math.atan2(this.x*v.y-this.y*v.x,this.dot(v))*R}
 rotate(angle){let c=Math.cos(angle*D),s=Math.sin(angle*D);return new Vec2(c*this.x-s*this.y,s*this.x+c*this.y)}
}
class Vec3 extends Vector {
 constructor(...a){super(3,a)}
 static fromSpherical(r,theta,phi){let t=theta*D,p=phi*D;return new Vec3(r*Math.sin(t)*Math.cos(p),r*Math.cos(t),r*Math.sin(t)*Math.sin(p))}
 toSpherical(){let r=this.length();return r===0?new Vec3(0):new Vec3(r,Math.acos(this.y/r)*R,Math.atan2(this.z,this.x)*R)}
 cross(v){return new Vec3(this.y*v.z-this.z*v.y,this.z*v.x-this.x*v.z,this.x*v.y-this.y*v.x)}
 refract(n,eta){let d=this.dot(n),k=1-eta*eta*(1-d*d);return k<0?new Vec3(0):this.multiply(eta).subtract(n.multiply(eta*d+Math.sqrt(k)))}
 angleBetween(v){return Math.acos(clamp(this.dot(v)/(this.length()*v.length()),-1,1))*R}
}
class Vec4 extends Vector {constructor(...a){super(4,a)}}
function det(rows){if(rows.length===1)return rows[0][0];let s=0;for(let c=0;c<rows.length;c++)s+=(c%2?-1:1)*rows[0][c]*det(rows.slice(1).map(r=>r.filter((_,i)=>i!==c)));return s}
class Matrix {
 constructor(n,source){Object.defineProperty(this,'_n',{value:n});this.m=source&&source.m?source.m.slice():Array.isArray(source)&&source.length===n*n?source.slice():typeof source==='string'&&source.split(' ').length===n*n?source.split(' ').map(parseFloat):Array.from({length:n*n},(_,i)=>i%(n+1)===0?1:0)}
 _new(m){let r=new this.constructor();r.m=m;return r} copy(){return this._new(this.m.slice())}
 add(other){return this._new(this.m.map((x,i)=>x+other.m[i]))} subtract(other){return this._new(this.m.map((x,i)=>x-other.m[i]))}
 multiply(value){let n=this._n;
  if(typeof value==='number')return this._new(this.m.map(x=>x*value));
  if(isVector(value)){let a=value._array();let b=Array.from({length:n},(_,r)=>a.reduce((s,x,c)=>s+this.m[c*n+r]*x,0));return new value.constructor(...b)}
  return this._new(Array.from({length:n*n},(_,i)=>{let r=i%n,c=Math.floor(i/n),s=0;for(let k=0;k<n;k++)s+=this.m[k*n+r]*value.m[c*n+k];return s}))
 }
 translate(v){return this.multiply(this.constructor.fromTranslation(v))}
 rotate(...a){return this.multiply(this.constructor.fromRotation(...a))} scale(v){return this.multiply(this.constructor.fromScale(v))}
 transpose(){let n=this._n;return this._new(this.m.map((_,i)=>this.m[(i%n)*n+Math.floor(i/n)]))}
 _rows(){let n=this._n,m=this.m,rows=[];for(let r=0;r<n;r++){let row=[];for(let c=0;c<n;c++)row.push(m[c*n+r]);rows.push(row)}return rows}
 determinant(){return det(this._rows())}
 inverse(){let rows=this._rows(),n=this._n,d=det(rows);return this._new(this.m.map((_,i)=>{let r=i%n,c=Math.floor(i/n);return ((r+c)%2?-1:1)*det(rows.filter((_,j)=>j!==c).map(row=>row.filter((_,j)=>j!==r)))/d}))}
 equals(o){return o!=null&&o._n===this._n&&Array.isArray(o.m)&&!this.m.some((x,i)=>Math.abs(x-o.m[i])>=1e-5)}
 toString(){return this.m.join(' ')}toConfigString(){return this.toString()}
}
class Mat3 extends Matrix {
 constructor(source){super(3,source)}static identity(){return new Mat3()}
 static fromTranslation(v){let r=new Mat3();r.m[6]=v.x;r.m[7]=v.y;return r}
 static fromScale(v){let r=new Mat3();r.m[0]=component(v,0);r.m[4]=component(v,1);return r}
 static fromRotation(angle){let r=new Mat3(),c=Math.cos(angle*D),s=Math.sin(angle*D);r.m[0]=c;r.m[1]=s;r.m[3]=-s;r.m[4]=c;return r}
 static fromBasis(right,up){let r=new Mat3();r.m[0]=right.x;r.m[1]=right.y;r.m[3]=up.x;r.m[4]=up.y;return r}
 static fromMat4(m){let r=new Mat3();r.m=[m.m[0],m.m[1],m.m[2],m.m[4],m.m[5],m.m[6],m.m[8],m.m[9],m.m[10]];return r}
 static compose(t,r,s){return Mat3.fromTranslation(t).multiply(Mat3.fromRotation(r)).multiply(Mat3.fromScale(s))}
 right(){return new Vec3(this.m[0],this.m[1],this.m[2])}up(){return new Vec3(this.m[3],this.m[4],this.m[5])}forward(){return new Vec3(this.m[6],this.m[7],this.m[8])}
 translation(v){if((isVector(v)&&v._n===2)){this.m[6]=v.x;this.m[7]=v.y;return this}return new Vec2(this.m[6],this.m[7])}
 angle(){return Math.atan2(this.m[0],-this.m[1])*R}
 transformPoint(v){let q=this.multiply(new Vec3(v.x,v.y,1));return new Vec2(q.x/q.z,q.y/q.z)}
 transformDirection(v){let q=this.multiply(new Vec3(v.x,v.y,0));return new Vec2(q.x,q.y)}
 decompose(){let m=this.m,sx=Math.hypot(m[0],m[1]),sy=Math.hypot(m[3],m[4]);if(m[0]*m[4]-m[3]*m[1]<0)sx=-sx;return {translation:this.translation(),rotation:sx!==0?Math.atan2(m[1]/sx,m[0]/sx)*R:0,scale:new Vec2(sx,sy)}}
}
class Mat4 extends Matrix {
 constructor(source){super(4,source)}static identity(){return new Mat4()}
 static fromTranslation(v){let r=new Mat4();r.m[12]=v.x;r.m[13]=v.y;r.m[14]=v.z??0;return r}
 static fromScale(v){let r=new Mat4();r.m[0]=component(v,0);r.m[5]=component(v,1);r.m[10]=component(v,2);return r}
 static fromRotation(angle,axis){axis=new Vec3(axis??new Vec3(0,0,1)).normalize();let x=axis.x,y=axis.y,z=axis.z,c=Math.cos(angle*D),s=Math.sin(angle*D),t=1-c;let r=new Mat4();r.m=[c+x*x*t,x*y*t+z*s,x*z*t-y*s,0,y*x*t-z*s,c+y*y*t,y*z*t+x*s,0,z*x*t+y*s,z*y*t-x*s,c+z*z*t,0,0,0,0,1];return r}
 static fromEuler(x,y,z){let v=(isVector(x)&&x._n===3)?x:{x,y,z},a=v.x*D,b=v.y*D,c=v.z*D,c1=Math.cos(-c),s1=Math.sin(-c),c2=Math.cos(-b),s2=Math.sin(-b),c3=Math.cos(-a),s3=Math.sin(-a),r=new Mat4();r.m=[c1*c2,-c2*s1,s2,0,c3*s1+c1*s2*s3,c1*c3-s1*s2*s3,-c2*s3,0,s1*s3-c1*c3*s2,c1*s3+c3*s1*s2,c2*c3,0,0,0,0,1];return r}
 static fromBasis(right,up,forward){let r=new Mat4();r.m=[right.x,right.y,right.z,0,up.x,up.y,up.z,0,forward.x,forward.y,forward.z,0,0,0,0,1];return r}
 static lookAt(eye,center,up){let z=eye.subtract(center).normalize(),x=up.cross(z).normalize(),y=z.cross(x),r=Mat4.fromBasis(x,y,z).transpose();r.m[12]=-x.dot(eye);r.m[13]=-y.dot(eye);r.m[14]=-z.dot(eye);return r}
 static compose(t,r,s){return Mat4.fromTranslation(t).multiply(Mat4.fromEuler(r)).multiply(Mat4.fromScale(s))}
 translation(v){if((isVector(v)&&v._n===3)||(isVector(v)&&v._n===2)){this.m[12]=v.x;this.m[13]=v.y;this.m[14]=(isVector(v)&&v._n===3)?v.z:0;return this}return new Vec3(this.m[12],this.m[13],this.m[14])}
 right(){return new Vec3(this.m[0],this.m[1],this.m[2])}up(){return new Vec3(this.m[4],this.m[5],this.m[6])}forward(){return new Vec3(this.m[8],this.m[9],this.m[10])}
 transformPoint(v){let q=this.multiply(new Vec4(v.x,v.y,v.z,1));return new Vec3(q.x/q.w,q.y/q.w,q.z/q.w)}
 transformDirection(v){let q=this.multiply(new Vec4(v.x,v.y,v.z,0));return new Vec3(q.x,q.y,q.z)}
 extractEuler(){let m=this.m,z=Math.atan2(m[1],m[0]),y=Math.atan2(-m[2],Math.hypot(m[6],m[10])),s=Math.sin(z),c=Math.cos(z),x=Math.atan2(s*m[8]-c*m[9],c*m[5]-s*m[4]);return new Vec3(x*R,y*R,z*R)}
 normalMatrix(){return Mat3.fromMat4(this).inverse().transpose()}
 decompose(){let t=this.translation(),s=new Vec3(this.right().length(),this.up().length(),this.forward().length());if(Mat3.fromMat4(this).determinant()<0)s.x=-s.x;let r=this.copy();for(let c=0;c<3;c++)for(let i=0;i<3;i++)r.m[c*4+i]/=component(s,c);return {translation:t,rotation:r.extractEuler(),scale:s}}
}
const WEMath={smoothStep:smooth,mix:(a,b,t)=>a+(b-a)*t,deg2rad:D,rad2deg:R};
const WEVector={angleVector2:a=>new Vec2(Math.cos(a*D),Math.sin(a*D)),vectorAngle2:v=>Math.atan2(v.y,v.x)*R};
const WEColor={
 normalizeColor:v=>new Vec3(v.x/255,v.y/255,v.z/255),expandColor:v=>new Vec3(v.x*255,v.y*255,v.z*255),
 rgb2hsv:v=>{let hi=Math.max(v.x,v.y,v.z),lo=Math.min(v.x,v.y,v.z),d=hi-lo,h=0;if(d!==0){h=hi===v.x?(v.y-v.z)/d+(v.y<v.z?6:0):hi===v.y?(v.z-v.x)/d+2:(v.x-v.y)/d+4;h/=6}return new Vec3(h,hi===0?0:d/hi,hi)},
 hsv2rgb:v=>{let i=Math.floor(v.x*6),f=v.x*6-i,p=v.z*(1-v.y),q=v.z*(1-f*v.y),t=v.z*(1-(1-f)*v.y),a=[[v.z,t,p],[q,v.z,p],[p,v.z,t],[p,q,v.z],[t,p,v.z],[v.z,p,q]][i%6];return a?new Vec3(...a):new Vec3()}
};
class MediaPlaybackEvent {constructor(value={}){Object.assign(this,value)}static PLAYBACK_STOPPED=0;static PLAYBACK_PLAYING=1;static PLAYBACK_PAUSED=2;get PLAYBACK_STOPPED(){return 0}get PLAYBACK_PLAYING(){return 1}get PLAYBACK_PAUSED(){return 2}}
Object.assign(G,{Vec2,Vec3,Vec4,Mat3,Mat4,WEMath,WEVector,WEColor,MediaPlaybackEvent});
})(globalThis);
