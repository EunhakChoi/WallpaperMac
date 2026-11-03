//! Materialize typed rows without dropping missing optional templates or repeated layers.
use super::{Issue,limits::Prepared};
use crate::{Error,Result};
use fbx_dom::{Document,LazyObject,Object,OwnedDocument,OwnedObject,Template};
use fbxscii::{Element,ElementAmphitheatre,ElementAttribute,SubTreeAttribute};
use std::{collections::{BTreeMap,BTreeSet,HashMap},io::Cursor};

pub struct Loaded {
    pub document:OwnedDocument,
    pub poses:BTreeMap<u64,Vec<ElementAttribute>>,
    pub extra_uv:BTreeMap<u64,Vec<String>>,
}
fn raw<'a>(doc:&'a Document,empty:&'a Template,lazy:&'a LazyObject)->Object<'a>{Object::new(doc,empty,lazy,0)}
fn subtree(doc:&Document,index:usize)->Result<ElementAttribute>{
    fn copy(doc:&Document,index:usize,parent:Option<usize>,arena:&mut ElementAmphitheatre,depth:usize)->Result<usize>{
        if depth>128{return Err(Error::limit(0,"FBX object attribute hierarchy too deep"))}
        let empty=Template::new();let lazy=LazyObject{name:String::new(),type_name:String::new(),class_name:String::new(),element_index:index};
        let object=raw(doc,&empty,&lazy);let source=object.element().ok_or_else(||Error::parse(0,"FBX object attribute index invalid"))?;
        let id=arena.insert(Element{key:source.key.clone(),tokens:source.tokens.clone(),children:Vec::new(),parent_index:parent});
        for child in &source.children{let child=copy(doc,*child,Some(id),arena,depth+1)?;arena.get_mut(id).unwrap().children.push(child);}
        Ok(id)
    }
    let mut amphitheatre=ElementAmphitheatre::new();let root_element_index=copy(doc,index,None,&mut amphitheatre,0)?;
    Ok(ElementAttribute::SubTree(Box::new(SubTreeAttribute{amphitheatre,root_element_index})))
}
pub fn load(input:&Prepared<'_>,issues:&mut Vec<Issue>)->Result<Loaded>{
    let settings=fbx_dom::ImportSettings{strict:false};
    let doc=if input.stats.binary {
        Document::from_binary_reader(Cursor::new(input.data.as_ref()),settings)
    }else{
        Document::from_parser(fbxscii::Parser::new(fbxscii::Tokenizer::new(Cursor::new(input.data.as_ref()))),settings)
    }.map_err(|e|Error::decode(0,format!("FBX document:{e:?}")))?;
    if !(7100..=7600).contains(&doc.version()){return Err(Error::unsupported(0,format!("FBX version{} outside7100..7600 profile",doc.version())))}
    let mut output=OwnedDocument{fbx_version:doc.version(),creator:doc.creator().into(),creation_date:*doc.creation_date(),global_settings:doc.global_settings().into(),..Default::default()};
    let mut poses=BTreeMap::new();let mut extra_uv=BTreeMap::new();let mut seen=BTreeSet::new();let mut missing_templates=BTreeSet::new();
    let empty=Template::new();
    // Object::new and its element accessor are public DOM APIs. Probe the bounded
    // object arena so an absent optional template cannot silently remove a row.
    let maximum=input.stats.nodes.saturating_add(input.stats.properties).saturating_add(16);
    for index in 0..maximum {
        let probe=LazyObject{name:String::new(),type_name:String::new(),class_name:String::new(),element_index:index};
        let object=raw(&doc,&empty,&probe);
        let Some(element)=object.element()else{break};
        if element.parent_index.is_some(){continue;}
        if element.tokens.len()<3{return Err(Error::parse(0,"FBX object row missing id/name/class"))}
        let id=element.tokens[0].parse::<u64>().map_err(|_|Error::unsupported(0,"negative/invalid FBX object id"))?;
        if !seen.insert(id){return Err(Error::parse(0,"duplicate FBX object id"))}
        if seen.len()>8192{return Err(Error::limit(0,"FBX object count exceeds8192 profile"))}
        let lazy=LazyObject{name:element.tokens[1].clone(),type_name:element.key.clone(),class_name:element.tokens[2].clone(),element_index:index};
        let fallback=Object::new(&doc,&empty,&lazy,id);
        let borrowed=if let Some(object)=doc.object_by_index(id){object}else{missing_templates.insert(lazy.type_name.clone());fallback};
        let mut attributes=HashMap::new();let mut pose_nodes=Vec::new();let mut uv_layers=Vec::new();
        for child in &element.children {
            let attr=subtree(&doc,*child)?;let key=attr.get_key().to_owned();
            if key=="Properties70"{continue;}
            if key=="PoseNode"{pose_nodes.push(attr.clone());}
            if key=="LayerElementUV"{uv_layers.push(attr.clone());}
            attributes.entry(key).or_insert(attr);
        }
        if !pose_nodes.is_empty(){poses.insert(id,pose_nodes);}
        if uv_layers.len()>1 {
            uv_layers.sort_by_key(|a|a.get_tokens().first().and_then(|n|n.parse::<u32>().ok()).unwrap_or(u32::MAX));
            let names=uv_layers.iter().map(|a|a.get_children_distinct().get("Name").and_then(|n|n.get_tokens().first()).cloned().unwrap_or_default()).collect::<Vec<_>>();
            extra_uv.insert(id,names);
            attributes.insert("LayerElementUV".into(),uv_layers.remove(0));
        }
        let properties=borrowed.properties();let connected_object_ids=borrowed.connected_object_ids().to_vec();let object_property_targets=borrowed.object_property_targets().to_vec();
        let owned=OwnedObject{object_index:id,name:lazy.name,type_name:lazy.type_name,class_name:lazy.class_name,properties,attributes,connected_object_ids,object_property_targets,pp_property_targets:HashMap::new()};
        use fbx_dom::ClassifiedFbxObject as C;
        let classified=C::try_from(owned).map_err(|e|Error::decode(0,format!("FBX object {}:{:?}",e.object.name,e.reason)))?;
        match classified {
            C::Model(v)=>output.models.push(v),C::MeshGeometry(v)=>output.mesh_geometries.push(v),C::LineGeometry(v)=>output.line_geometries.push(v),C::ShapeGeometry(v)=>output.shape_geometries.push(v),C::UnknownGeometry(v)=>output.unknown_geometries.push(v),
            C::Camera(v)=>output.cameras.push(v),C::CameraSwitcher(v)=>output.camera_switchers.push(v),C::Light(v)=>output.lights.push(v),C::NullNode(v)=>output.null_nodes.push(v),C::LimbNode(v)=>output.limb_nodes.push(v),C::UnknownNodeAttribute(v)=>output.unknown_node_attributes.push(v),
            C::Material(v)=>output.materials.push(v),C::Texture(v)=>output.textures.push(v),C::LayeredTexture(v)=>output.layered_textures.push(v),C::Video(v)=>output.videos.push(v),C::Cluster(v)=>output.clusters.push(v),C::Skin(v)=>output.skins.push(v),C::BlendShape(v)=>output.blend_shapes.push(v),C::BlendShapeChannel(v)=>output.blend_shape_channels.push(v),C::UnknownDeformer(v)=>output.unknown_deformers.push(v),
            C::AnimationStack(v)=>output.animation_stacks.push(v),C::AnimationLayer(v)=>output.animation_layers.push(v),C::AnimationCurve(v)=>output.animation_curves.push(v),C::AnimationCurveNode(v)=>output.animation_curve_nodes.push(v),C::Unknown(v)=>output.unknown_objects.push(v),
        }
    }
    for name in missing_templates {issues.push(Issue::info("optional-template",format!("{name}: no optional property template; explicit properties and standard transform defaults are used")));}
    output.models.sort_by_key(|v|v.inner().object_index);output.mesh_geometries.sort_by_key(|v|v.inner().object_index);output.animation_stacks.sort_by_key(|v|v.inner().object_index);output.animation_curves.sort_by_key(|v|v.inner().object_index);
    Ok(Loaded{document:output,poses,extra_uv})
}
