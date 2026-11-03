//! Copy uniform array elements into natural-layout by-value function arguments.
//! Naga distinguishes std140 array strides from ordinary parameter array strides.
use crate::{Error, Result};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
struct ArrayType {
    element: String,
    count: usize,
}
#[derive(Clone)]
struct Parameter {
    array: Option<ArrayType>,
    writable: bool,
}
fn identifier(token: &str) -> bool {
    token
        .bytes()
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
fn count(token: &str) -> Option<usize> {
    token.trim_end_matches(['u', 'U']).parse().ok()
}
fn array_declaration(tokens: &[String]) -> Option<(String, ArrayType)> {
    // Deliberately retain other/multidimensional declarations for Naga rather
    // than guessing their layout, size expressions or resource semantics.
    let tokens = tokens.strip_prefix(&["const".into()]).unwrap_or(tokens);
    if tokens.len() != 6
        || !identifier(&tokens[0])
        || !identifier(&tokens[1])
        || tokens[2] != "["
        || tokens[4] != "]"
        || tokens[5] != ";"
    {
        return None;
    }
    Some((
        tokens[1].clone(),
        ArrayType {
            element: tokens[0].clone(),
            count: count(&tokens[3])?,
        },
    ))
}
/// Return each comma-delimited argument range, preserving nested calls/indexing.
fn arguments(tokens: &[String], open: usize) -> Option<(usize, Vec<std::ops::Range<usize>>)> {
    if tokens.get(open)? != "(" {
        return None;
    }
    let mut parens = 1;
    let mut brackets = 0;
    let mut start = open + 1;
    let mut result = Vec::new();
    for (end, token) in tokens.iter().enumerate().skip(open + 1) {
        match token.as_str() {
            "(" => parens += 1,
            ")" => {
                parens -= 1;
                if parens == 0 {
                    if end != start {
                        result.push(start..end);
                    }
                    return Some((end, result));
                }
            }
            "[" => brackets += 1,
            "]" => brackets -= 1,
            "," if parens == 1 && brackets == 0 => {
                result.push(start..end);
                start = end + 1;
            }
            _ => {}
        }
    }
    None
}

pub(crate) fn lower(tokens: &[String]) -> Result<Vec<String>> {
    let mut uniforms = BTreeMap::new();
    let mut uniform_names = BTreeSet::new();
    let mut functions: BTreeMap<String, Vec<Vec<Parameter>>> = BTreeMap::new();
    let mut braces = 0i32;
    let mut parens = 0i32;
    for i in 0..tokens.len() {
        if braces == 0 && parens == 0 {
            if tokens[i] == "uniform" {
                if let Some(end) = tokens[i..].iter().position(|t| t == ";") {
                    let declaration = &tokens[i + 1..=i + end];
                    if let Some((name, ty)) = array_declaration(declaration) {
                        uniform_names.insert(i + 2);
                        uniforms.insert(name, ty);
                    }
                }
            } else if identifier(&tokens[i])
                && i > 0
                && identifier(&tokens[i - 1])
                && tokens.get(i + 1).is_some_and(|t| t == "(")
                && let Some((end, args)) = arguments(tokens, i + 1)
                && tokens.get(end + 1).is_some_and(|t| t == "{" || t == ";")
            {
                let params = args
                    .into_iter()
                    .map(|range| {
                        let raw = &tokens[range];
                        let writable = raw.iter().any(|t| t == "out" || t == "inout");
                        let mut declaration = raw
                            .iter()
                            .filter(|t| {
                                !matches!(
                                    t.as_str(),
                                    "in" | "out" | "inout" | "const" | "highp" | "mediump" | "lowp"
                                )
                            })
                            .cloned()
                            .collect::<Vec<_>>();
                        declaration.push(";".into());
                        Parameter {
                            array: array_declaration(&declaration).map(|(_, ty)| ty),
                            writable,
                        }
                    })
                    .collect::<Vec<_>>();
                if params.iter().any(|p| p.array.is_some()) {
                    functions.entry(tokens[i].clone()).or_default().push(params);
                }
            }
        }
        match tokens[i].as_str() {
            "{" => braces += 1,
            "}" => braces -= 1,
            "(" => parens += 1,
            ")" => parens -= 1,
            _ => {}
        }
    }
    // A local/parameter sharing a uniform name needs proper scope resolution;
    // leave that name untouched instead of accidentally copying another value.
    let mut shadowed = BTreeSet::new();
    for i in 1..tokens.len() {
        if uniforms.contains_key(&tokens[i])
            && !uniform_names.contains(&i)
            && identifier(&tokens[i - 1])
            && !matches!(tokens[i - 1].as_str(), "return" | "case")
        {
            shadowed.insert(tokens[i].clone());
        }
    }
    let mut copies = BTreeMap::new();
    for i in 0..tokens.len().saturating_sub(1) {
        let Some(overloads) = functions.get(&tokens[i]) else {
            continue;
        };
        if tokens[i + 1] != "(" {
            continue;
        }
        let Some((_, args)) = arguments(tokens, i + 1) else {
            continue;
        };
        for (index, range) in args.iter().enumerate() {
            if range.len() != 1 {
                continue;
            }
            let name = &tokens[range.start];
            let Some(ty) = uniforms.get(name).filter(|_| !shadowed.contains(name)) else {
                continue;
            };
            let matching = overloads
                .iter()
                .filter(|p| p.len() == args.len() && p[index].array.as_ref() == Some(ty))
                .collect::<Vec<_>>();
            if matching.is_empty() || matching.iter().any(|p| p[index].writable) {
                continue;
            }
            if ty.count == 0 || ty.count > 4096 {
                return Err(Error::limit(
                    0,
                    "uniform array function argument exceeds4096element copy policy",
                ));
            }
            copies.insert(range.start, (name.clone(), ty.clone()));
        }
    }
    let mut out = Vec::with_capacity(tokens.len());
    for (i, token) in tokens.iter().enumerate() {
        if let Some((name, ty)) = copies.get(&i) {
            if out.len().saturating_add(ty.count * 5 + 5) > 1_000_000 {
                return Err(Error::limit(
                    0,
                    "uniform array argument expansion exceeds token policy",
                ));
            }
            out.extend([
                ty.element.clone(),
                "[".into(),
                ty.count.to_string(),
                "]".into(),
                "(".into(),
            ]);
            for n in 0..ty.count {
                if n > 0 {
                    out.push(",".into());
                }
                out.extend([name.clone(), "[".into(), n.to_string(), "]".into()]);
            }
            out.push(")".into());
        } else {
            out.push(token.clone());
        }
        if out.len() > 1_000_000 {
            return Err(Error::limit(
                0,
                "uniform array argument expansion exceeds token policy",
            ));
        }
    }
    Ok(out)
}
