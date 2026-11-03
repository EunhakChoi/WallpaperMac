use crate::{Error, Result};
use serde::Serialize;
use std::{borrow::Cow, io::Cursor};

pub const MAX_FILE: usize = 128 * 1024 * 1024;
const MAX_ASCII: usize = 16 * 1024 * 1024;
const MAX_NODES: usize = 250_000;
const MAX_DEPTH: usize = 128;
const MAX_TOKENS: usize = 1_000_000;
const MAX_DOM_ESTIMATE: usize = 192 * 1024 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct Connection {
    pub kind: String,
    pub source: u64,
    pub destination: u64,
    pub property: Option<String>,
}
#[derive(Clone, Debug, Serialize, Default)]
pub struct Stats {
    pub binary: bool,
    pub version: Option<u32>,
    pub nodes: usize,
    pub properties: usize,
    pub decoded_array_bytes: usize,
    pub estimated_dom_bytes: usize,
    pub maximum_depth: usize,
}
pub struct Prepared<'a> {
    pub data: Cow<'a, [u8]>,
    pub stats: Stats,
    pub connections: Vec<Connection>,
}
fn range(data: &[u8], offset: usize, length: usize) -> Result<&[u8]> {
    let end=offset.checked_add(length).ok_or_else(||Error::limit(offset,"FBX offset overflow"))?;
    data.get(offset..end).ok_or_else(||Error::parse(offset,"truncated FBX record"))
}
fn u32_at(data:&[u8],offset:usize)->Result<u32>{Ok(u32::from_le_bytes(range(data,offset,4)?.try_into().unwrap()))}
fn u64_at(data:&[u8],offset:usize)->Result<u64>{Ok(u64::from_le_bytes(range(data,offset,8)?.try_into().unwrap()))}
fn charge(stats:&mut Stats, bytes:usize, offset:usize)->Result<()> {
    stats.estimated_dom_bytes=stats.estimated_dom_bytes.checked_add(bytes).ok_or_else(||Error::limit(offset,"FBX allocation estimate overflow"))?;
    if stats.estimated_dom_bytes>MAX_DOM_ESTIMATE{return Err(Error::limit(offset,"FBX decoded DOM exceeds memory policy"))}
    Ok(())
}
fn connection(values:&[String])->Result<Option<Connection>>{
    if values.len()<3{return Ok(None)}
    let kind=values[0].trim_matches('"');
    if !matches!(kind,"OO"|"OP"|"PP"){return Ok(None)}
    let id=|s:&str|s.trim().parse::<u64>().map_err(|_|Error::unsupported(0,"negative or invalid FBX object connection id"));
    Ok(Some(Connection{kind:kind.into(),source:id(&values[1])?,destination:id(&values[2])?,property:values.get(3).cloned()}))
}
fn binary_node(data:&[u8],start:usize,parent_end:usize,version:u32,depth:usize,parent:&str,stats:&mut Stats,connections:&mut Vec<Connection>)->Result<usize>{
    let wide=version>=7500;let header=if wide{25}else{13};
    let read=|at|if wide{u64_at(data,at)}else{u32_at(data,at).map(u64::from)};
    let end=usize::try_from(read(start)?).map_err(|_|Error::limit(start,"FBX node end exceeds address space"))?;
    let properties=usize::try_from(read(start+if wide{8}else{4})?).map_err(|_|Error::limit(start,"FBX property count overflow"))?;
    let property_bytes=usize::try_from(read(start+if wide{16}else{8})?).map_err(|_|Error::limit(start,"FBX property byte count overflow"))?;
    let name_len=range(data,start+header-1,1)?[0]as usize;
    let name=std::str::from_utf8(range(data,start+header,name_len)?).map_err(|_|Error::parse(start,"FBX node name is not UTF-8"))?;
    let begin=start+header+name_len;
    let property_end=begin.checked_add(property_bytes).ok_or_else(||Error::limit(start,"FBX property range overflow"))?;
    if end<=start||end>parent_end||property_end>end{return Err(Error::parse(start,"invalid FBX node/property extent"))}
    stats.nodes+=1;stats.properties=stats.properties.checked_add(properties).ok_or_else(||Error::limit(start,"FBX property count overflow"))?;
    stats.maximum_depth=stats.maximum_depth.max(depth);
    if depth>MAX_DEPTH||stats.nodes>MAX_NODES||stats.properties>MAX_TOKENS{return Err(Error::limit(start,"FBX depth/node/property limit"))}
    charge(stats,128usize.saturating_add(properties.saturating_mul(64)),start)?;
    let record_connection=name=="C"&&parent=="Connections";
    let mut values=Vec::new();let mut at=begin;
    for _ in 0..properties {
        let tag=range(data,at,1)?[0];at+=1;
        let scalar=match tag{b'Y'=>Some(2),b'C'=>Some(1),b'I'|b'F'=>Some(4),b'L'|b'D'=>Some(8),_=>None};
        if let Some(length)=scalar {
            let bytes=range(data,at,length)?;
            if record_connection {
                let value=match tag{b'L'=>i64::from_le_bytes(bytes.try_into().unwrap()).to_string(),b'I'=>i32::from_le_bytes(bytes.try_into().unwrap()).to_string(),_=>return Err(Error::parse(at,"FBX connection has noninteger id"))};
                values.push(value);
            }
            at+=length;
        }else if tag==b'S'||tag==b'R' {
            let length=u32_at(data,at)?as usize;at+=4;let bytes=range(data,at,length)?;
            charge(stats,length,at)?;
            if record_connection&&tag==b'S'{values.push(std::str::from_utf8(bytes).map_err(|_|Error::parse(at,"invalid FBX connection string"))?.to_owned());}
            at+=length;
        }else if matches!(tag,b'f'|b'd'|b'l'|b'i'|b'b'|b'c') {
            let count=u32_at(data,at)?as usize;let encoding=u32_at(data,at+4)?;let stored=u32_at(data,at+8)?as usize;at+=12;
            let element=match tag{b'd'|b'l'=>8,b'f'|b'i'=>4,_=>1};
            let decoded=count.checked_mul(element).ok_or_else(||Error::limit(at,"FBX array size overflow"))?;
            stats.decoded_array_bytes=stats.decoded_array_bytes.checked_add(decoded).ok_or_else(||Error::limit(at,"FBX decoded array sum overflow"))?;
            charge(stats,count.checked_mul(64).ok_or_else(||Error::limit(at,"FBX array estimate overflow"))?,at)?;
            if encoding>1{return Err(Error::unsupported(at,"unknown FBX array encoding"))}
            if encoding==0&&stored!=decoded{return Err(Error::parse(at,"FBX direct array length mismatch"))}
            if encoding==1&&stored==0&&count!=0{return Err(Error::parse(at,"empty FBX compressed array"))}
            range(data,at,stored)?;at+=stored;
        }else{return Err(Error::unsupported(at-1,format!("unknown FBX property tag {tag:#x}")))}
        if at>property_end{return Err(Error::parse(at,"FBX property exceeds declared bytes"))}
    }
    if at!=property_end{return Err(Error::parse(at,"FBX property count/byte length mismatch"))}
    if record_connection&&let Some(c)=connection(&values)?{connections.push(c);}
    while at<end {
        if range(data,at,header)?.iter().all(|v|*v==0){at+=header;break}
        at=binary_node(data,at,end,version,depth+1,name,stats,connections)?;
    }
    if at!=end{return Err(Error::parse(at,"FBX child list exceeds node extent"))}
    Ok(end)
}

pub fn prepare(data:&[u8])->Result<Prepared<'_>>{
    if data.len()>MAX_FILE{return Err(Error::limit(0,"FBX input exceeds128MiB policy"))}
    if data.starts_with(b"Kaydara FBX Binary  \0\x1a\0") {
        let version=u32_at(data,23)?;
        if !(7100..=7600).contains(&version){return Err(Error::unsupported(23,format!("FBX binary version{version}; supported profile7100..7600")))}
        let mut stats=Stats{binary:true,version:Some(version),..Default::default()};let mut connections=Vec::new();let header=if version>=7500{25}else{13};let mut at=27;
        loop{
            if range(data,at,header)?.iter().all(|v|*v==0){break}
            at=binary_node(data,at,data.len(),version,1,"",&mut stats,&mut connections)?;
        }
        return Ok(Prepared{data:Cow::Borrowed(data),stats,connections})
    }
    if data.len()>MAX_ASCII{return Err(Error::limit(0,"ASCII FBX input exceeds16MiB policy"))}
    let source=std::str::from_utf8(data).map_err(|_|Error::parse(0,"ASCII FBX is not UTF-8"))?;
    // Bound the upstream tokenizer's recursive whitespace/comment skipping.
    // Quoted text is retained byte for byte; external whitespace is collapsed.
    let mut normalized=String::with_capacity(source.len());let mut quoted=false;let mut comment=false;let mut space=false;
    for ch in source.chars(){
        if comment{if ch=='\n'{comment=false;space=true;}continue}
        if !quoted&&ch==';'{comment=true;continue}
        if !quoted&&ch.is_whitespace(){space=true;continue}
        if space&&!normalized.is_empty(){normalized.push(' ');}space=false;
        if quoted&&ch=='\\'{return Err(Error::unsupported(0,"ASCII FBX escaped strings require an escape-aware tokenizer"))}
        if ch=='"'{quoted=!quoted;}
        normalized.push(ch);
    }
    if quoted{return Err(Error::parse(0,"unterminated ASCII FBX string"))}
    normalized.push('\n');
    let mut stats=Stats::default();let mut connections=Vec::new();let mut stack=Vec::new();let mut pending=String::new();let mut values=Vec::new();
    let finish=|key:&str,values:&[String],stack:&[String],connections:&mut Vec<Connection>|->Result<()>{if key=="C"&&stack.last().is_some_and(|p|p=="Connections")&&let Some(c)=connection(values)?{connections.push(c);}Ok(())};
    for token in fbxscii::Tokenizer::new(Cursor::new(normalized.as_bytes())){
        let token=token.map_err(|e|Error::parse(0,format!("ASCII FBX token:{e:?}")))?;
        stats.properties+=1;
        if stats.properties>MAX_TOKENS{return Err(Error::limit(0,"ASCII FBX token limit"))}
        match token.data {
            fbxscii::Token::Key(key)=>{finish(&pending,&values,&stack,&mut connections)?;pending=key;values.clear();stats.nodes+=1;if stats.nodes>MAX_NODES{return Err(Error::limit(0,"ASCII FBX node limit"))}}
            fbxscii::Token::Data(value)=>{charge(&mut stats,value.len().saturating_add(64),0)?;if pending=="C"{values.push(value);} }
            fbxscii::Token::OpenBrace=>{stack.push(pending.clone());pending.clear();values.clear();stats.maximum_depth=stats.maximum_depth.max(stack.len());if stack.len()>MAX_DEPTH{return Err(Error::limit(0,"ASCII FBX hierarchy too deep"))}}
            fbxscii::Token::CloseBrace=>{finish(&pending,&values,&stack,&mut connections)?;if stack.pop().is_none(){return Err(Error::parse(0,"unmatched ASCII FBX brace"))}pending.clear();values.clear();}
            fbxscii::Token::Comma=>{}
        }
    }
    finish(&pending,&values,&stack,&mut connections)?;
    if !stack.is_empty(){return Err(Error::parse(0,"unterminated ASCII FBX node"))}
    Ok(Prepared{data:Cow::Owned(normalized.into_bytes()),stats,connections})
}
