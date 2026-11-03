use crate::{Error,Result};
pub type Matrix=[f64;16];
pub fn identity()->Matrix{[1.,0.,0.,0.,0.,1.,0.,0.,0.,0.,1.,0.,0.,0.,0.,1.]}
pub fn mul(a:Matrix,b:Matrix)->Matrix{std::array::from_fn(|i|{let c=i/4;let r=i%4;(0..4).map(|k|a[k*4+r]*b[c*4+k]).sum()})}
pub fn translate(v:[f64;3])->Matrix{let mut m=identity();m[12..15].copy_from_slice(&v);m}
pub fn scale(v:[f64;3])->Matrix{let mut m=identity();for i in 0..3{m[i*5]=v[i];}m}
pub fn inverse(m:Matrix)->Result<Matrix>{
    let mut a=[[0.;8];4];for r in 0..4{for c in 0..4{a[r][c]=m[c*4+r];}a[r][4+r]=1.;}
    for c in 0..4 {
        let pivot=(c..4).max_by(|i,j|a[*i][c].abs().total_cmp(&a[*j][c].abs())).unwrap();
        if a[pivot][c]==0.||!a[pivot][c].is_finite(){return Err(Error::unsupported(0,"singular FBX bind/transform matrix"))}
        a.swap(c,pivot);let d=a[c][c];for j in 0..8{a[c][j]/=d;}
        for r in 0..4{if r==c{continue;}let d=a[r][c];for j in 0..8{a[r][j]-=d*a[c][j];}}
    }
    let result=std::array::from_fn(|i|a[i%4][4+i/4]);
    if result.iter().any(|v|!v.is_finite()){return Err(Error::parse(0,"nonfinite inverse FBX matrix"))}Ok(result)
}
pub fn euler(degrees:[f64;3],order:i32)->Result<Matrix>{
    let axes=match order{0=>[0,1,2],1=>[0,2,1],2=>[1,2,0],3=>[1,0,2],4=>[2,0,1],5=>[2,1,0],_=>return Err(Error::unsupported(0,"FBX spherical/unknown Euler order"))};
    let mut result=identity();
    for axis in axes {
        let (s,c)=degrees[axis].to_radians().sin_cos();let mut m=identity();
        match axis{0=>{m[5]=c;m[6]=s;m[9]=-s;m[10]=c;},1=>{m[0]=c;m[2]=-s;m[8]=s;m[10]=c;},_=>{m[0]=c;m[1]=s;m[4]=-s;m[5]=c;}}
        result=mul(m,result);
    }
    Ok(result)
}
pub fn point(m:Matrix,p:[f32;3])->Result<[f32;3]>{
    finite3(std::array::from_fn(|r|m[r]*p[0]as f64+m[4+r]*p[1]as f64+m[8+r]*p[2]as f64+m[12+r]))
}
pub fn direction(m:Matrix,p:[f32;3])->[f32;3]{normalize(std::array::from_fn(|r|(m[r]*p[0]as f64+m[4+r]*p[1]as f64+m[8+r]*p[2]as f64)as f32))}
pub fn normal(inverse:Matrix,p:[f32;3])->[f32;3]{normalize(std::array::from_fn(|r|(inverse[r*4]*p[0]as f64+inverse[r*4+1]*p[1]as f64+inverse[r*4+2]*p[2]as f64)as f32))}
pub fn finite(m:Matrix)->Result<[f32;16]>{let v=m.map(|v|v as f32);if v.iter().any(|v|!v.is_finite()){return Err(Error::parse(0,"FBX transform exceeds finite float32 range"))}Ok(v)}
pub fn finite3(m:[f64;3])->Result<[f32;3]>{let v=m.map(|v|v as f32);if v.iter().any(|v|!v.is_finite()){return Err(Error::parse(0,"nonfinite FBX vector"))}Ok(v)}
pub fn normalize(v:[f32;3])->[f32;3]{let length=v.iter().map(|v|(*v as f64).powi(2)).sum::<f64>().sqrt();if length==0.{[0.,0.,1.]}else{v.map(|v|(v as f64/length)as f32)}}
pub fn cross(a:[f32;3],b:[f32;3])->[f32;3]{[a[1]*b[2]-a[2]*b[1],a[2]*b[0]-a[0]*b[2],a[0]*b[1]-a[1]*b[0]]}
pub fn determinant(m:Matrix)->f64{m[0]*(m[5]*m[10]-m[9]*m[6])-m[4]*(m[1]*m[10]-m[9]*m[2])+m[8]*(m[1]*m[6]-m[5]*m[2])}
pub fn close(a:Matrix,b:Matrix)->bool{a.iter().zip(b).all(|(a,b)|(a-b).abs()<=1e-4*(1.+a.abs().max(b.abs())))}
pub fn uniform_scale(m:Matrix)->bool{
    let lengths=(0..3).map(|c|(0..3).map(|r|m[c*4+r].powi(2)).sum::<f64>().sqrt()).collect::<Vec<_>>();
    let tolerance=1e-5*(1.+lengths.iter().copied().fold(0.,f64::max));
    (lengths[0]-lengths[1]).abs()<tolerance&&(lengths[1]-lengths[2]).abs()<tolerance
        &&(0..3).all(|a|(a+1..3).all(|b|(0..3).map(|r|m[a*4+r]*m[b*4+r]).sum::<f64>().abs()<tolerance))
}
pub fn unit_scale(m:Matrix)->bool{uniform_scale(m)&&((0..3).map(|r|m[r]*m[r]).sum::<f64>()-1.).abs()<1e-5}
