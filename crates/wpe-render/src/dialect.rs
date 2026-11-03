//! Explicit HLSL-style scalar/vector broadcast and truncation for the shipped GLSL dialect.
//! Unknown expressions remain unchanged and must pass Naga's normal validator.
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Ty {
    Scalar(String),
    Vector(u8, String),
    Matrix(u8, u8),
    Array(Box<Ty>),
    Unknown,
}
impl Ty {
    fn parse(s: &str) -> Option<Self> {
        if ["float", "int", "uint", "bool"].contains(&s) {
            return Some(Self::Scalar(s.into()));
        }
        for (prefix, kind) in [
            ("vec", "float"),
            ("ivec", "int"),
            ("uvec", "uint"),
            ("bvec", "bool"),
        ] {
            if let Some(n) = s
                .strip_prefix(prefix)
                .and_then(|s| s.parse::<u8>().ok())
                .filter(|n| (2..=4).contains(n))
            {
                return Some(Self::Vector(n, kind.into()));
            }
        }
        if let Some(m) = s.strip_prefix("mat") {
            let mut p = m.split('x');
            let c = p.next()?.parse::<u8>().ok()?;
            let r = p.next().and_then(|s| s.parse().ok()).unwrap_or(c);
            return Some(Self::Matrix(c, r));
        }
        None
    }
    fn glsl(&self) -> Option<String> {
        match self {
            Self::Scalar(k) => Some(k.clone()),
            Self::Vector(n, k) => Some(format!(
                "{}{n}",
                match k.as_str() {
                    "int" => "ivec",
                    "uint" => "uvec",
                    "bool" => "bvec",
                    _ => "vec",
                }
            )),
            Self::Matrix(c, r) => Some(format!("mat{c}x{r}")),
            _ => None,
        }
    }
}
#[derive(Clone)]
struct Expr {
    code: String,
    ty: Ty,
    value: Option<Vec<f32>>,
}
struct Parser<'a> {
    tokens: &'a [String],
    pos: usize,
    symbols: &'a BTreeMap<String, Ty>,
    functions: &'a BTreeMap<String, Ty>,
    parameters: &'a BTreeMap<String, Vec<(Ty, bool)>>,
    constants: &'a BTreeMap<String, Vec<f32>>,
}
fn coerce(mut e: Expr, ty: &Ty) -> Expr {
    if e.ty == *ty {
        return e;
    }
    match (ty, &e.ty) {
        (Ty::Scalar(kind), Ty::Vector(_, other)) => {
            e.code = format!("({}).x", e.code);
            if kind != other {
                e.code = format!("{kind}({})", e.code);
            }
            e.value = e.value.map(|v| vec![v[0]]);
            e.ty = ty.clone();
        }
        (Ty::Vector(n, _), Ty::Vector(m, _)) if m > n => {
            e.code = format!("{}({})", ty.glsl().unwrap(), e.code);
            e.value = e.value.map(|mut v| {
                v.truncate(*n as usize);
                v
            });
            e.ty = ty.clone();
        }
        (Ty::Vector(n, _), Ty::Scalar(_)) => {
            e.code = format!("{}({})", ty.glsl().unwrap(), e.code);
            e.value = e.value.map(|v| vec![v[0]; *n as usize]);
            e.ty = ty.clone();
        }
        (Ty::Scalar(_), Ty::Scalar(_)) => {
            e.code = format!("{}({})", ty.glsl().unwrap(), e.code);
            e.ty = ty.clone();
        }
        _ => {}
    }
    e
}
fn common(a: &Ty, b: &Ty) -> Ty {
    match (a, b) {
        (Ty::Vector(n, k), Ty::Vector(m, _)) => Ty::Vector((*n).min(*m), k.clone()),
        (Ty::Vector(..), Ty::Scalar(_)) => a.clone(),
        (Ty::Scalar(_), Ty::Vector(..)) => b.clone(),
        (Ty::Scalar(a), Ty::Scalar(b)) => Ty::Scalar(if a == "float" || b == "float" {
            "float".into()
        } else {
            a.clone()
        }),
        _ => Ty::Unknown,
    }
}
fn precedence(s: &str) -> Option<u8> {
    Some(match s {
        "||" => 1,
        "^^" => 2,
        "&&" => 3,
        "|" => 4,
        "^" => 5,
        "&" => 6,
        "==" | "!=" => 7,
        "<" | ">" | "<=" | ">=" => 8,
        "<<" | ">>" => 9,
        "+" | "-" => 10,
        "*" | "/" | "%" => 11,
        _ => return None,
    })
}
impl Parser<'_> {
    fn parse(&mut self, min: u8) -> Option<Expr> {
        let mut left = self.primary()?;
        while let Some(op) = self.tokens.get(self.pos).cloned() {
            if op == "?" && min == 0 {
                self.pos += 1;
                let yes = self.parse(0)?;
                if self.tokens.get(self.pos)? != ":" {
                    return None;
                }
                self.pos += 1;
                let no = self.parse(0)?;
                let ty = common(&yes.ty, &no.ty);
                let y = coerce(yes, &ty);
                let n = coerce(no, &ty);
                left = Expr {
                    code: format!("({}?{}:{})", left.code, y.code, n.code),
                    ty,
                    value: None,
                };
                continue;
            }
            let Some(p) = precedence(&op) else {
                break;
            };
            if p < min {
                break;
            }
            self.pos += 1;
            let mut right = self.parse(p + 1)?;
            let ty = if op == "*" {
                match (&left.ty, &right.ty) {
                    (Ty::Matrix(_, r), Ty::Vector(..)) => Ty::Vector(*r, "float".into()),
                    (Ty::Vector(..), Ty::Matrix(c, _)) => Ty::Vector(*c, "float".into()),
                    (Ty::Matrix(_, r), Ty::Matrix(c, _)) => Ty::Matrix(*c, *r),
                    _ => common(&left.ty, &right.ty),
                }
            } else {
                common(&left.ty, &right.ty)
            };
            if matches!((&left.ty, &right.ty), (Ty::Vector(..), Ty::Vector(..))) {
                left = coerce(left, &ty);
                right = coerce(right, &ty);
            }
            let value = match (&left.value, &right.value) {
                (Some(a), Some(b)) if ["+", "-", "*", "/"].contains(&op.as_str()) => {
                    let n = a.len().max(b.len());
                    if (a.len() != 1 && a.len() != n) || (b.len() != 1 && b.len() != n) {
                        None
                    } else {
                        Some(
                            (0..n)
                                .map(|i| {
                                    let a = a[if a.len() == 1 { 0 } else { i }];
                                    let b = b[if b.len() == 1 { 0 } else { i }];
                                    match op.as_str() {
                                        "+" => a + b,
                                        "-" => a - b,
                                        "*" => a * b,
                                        _ => a / b,
                                    }
                                })
                                .collect(),
                        )
                    }
                }
                _ => None,
            };
            left = Expr {
                code: format!("({} {op} {})", left.code, right.code),
                ty: if p <= 3 || p == 7 || p == 8 {
                    Ty::Scalar("bool".into())
                } else {
                    ty
                },
                value,
            };
        }
        Some(left)
    }
    fn primary(&mut self) -> Option<Expr> {
        let token = self.tokens.get(self.pos)?.clone();
        self.pos += 1;
        let mut value = if ["-", "+", "!", "~", "++", "--"].contains(&token.as_str()) {
            let e = self.primary()?;
            Expr {
                code: format!("({token}{})", e.code),
                ty: e.ty,
                value: e.value.map(|v| {
                    v.into_iter()
                        .map(|x| if token == "-" { -x } else { x })
                        .collect()
                }),
            }
        } else if token == "(" {
            let e = self.parse(0)?;
            if self.tokens.get(self.pos)? != ")" {
                return None;
            }
            self.pos += 1;
            Expr {
                code: format!("({})", e.code),
                ..e
            }
        } else if self.tokens.get(self.pos).is_some_and(|s| s == "(") {
            self.pos += 1;
            let mut args = Vec::new();
            if self.tokens.get(self.pos).is_some_and(|s| s != ")") {
                loop {
                    args.push(self.parse(0)?);
                    match self.tokens.get(self.pos)?.as_str() {
                        "," => self.pos += 1,
                        ")" => break,
                        _ => return None,
                    }
                }
            }
            if self.tokens.get(self.pos)? != ")" {
                return None;
            }
            self.pos += 1;
            self.call(&token, args)
        } else {
            let constant = token
                .trim_end_matches(['u', 'U'])
                .parse::<f32>()
                .ok()
                .map(|n| vec![n])
                .or_else(|| self.constants.get(&token).cloned());
            let ty = self.symbols.get(&token).cloned().unwrap_or_else(|| {
                if token.parse::<f32>().is_ok() {
                    Ty::Scalar(
                        if token.contains(['.', 'e', 'E']) {
                            "float"
                        } else {
                            "int"
                        }
                        .into(),
                    )
                } else {
                    Ty::Unknown
                }
            });
            Expr {
                code: token,
                ty,
                value: constant,
            }
        };
        loop {
            match self.tokens.get(self.pos).map(String::as_str) {
                Some(".") => {
                    let field = self.tokens.get(self.pos + 1)?.clone();
                    self.pos += 2;
                    let indices = field
                        .chars()
                        .map(|c| match c {
                            'x' | 'r' | 's' => Some(0),
                            'y' | 'g' | 't' => Some(1),
                            'z' | 'b' | 'p' => Some(2),
                            'w' | 'a' | 'q' => Some(3),
                            _ => None,
                        })
                        .collect::<Option<Vec<_>>>();
                    if let Some(indices) = indices {
                        if indices.is_empty() || indices.len() > 4 {
                            return None;
                        }
                        let kind = match &value.ty {
                            Ty::Vector(_, k) => k.clone(),
                            _ => "float".into(),
                        };
                        value.ty = if indices.len() == 1 {
                            Ty::Scalar(kind)
                        } else {
                            Ty::Vector(indices.len() as u8, kind)
                        };
                        value.value = value
                            .value
                            .and_then(|v| indices.iter().map(|i| v.get(*i).copied()).collect());
                    } else {
                        value.ty = Ty::Unknown;
                        value.value = None;
                    }
                    value.code = format!("({}).{field}", value.code);
                }
                Some("[") => {
                    self.pos += 1;
                    let index = self.parse(0)?;
                    if self.tokens.get(self.pos)? != "]" {
                        return None;
                    }
                    self.pos += 1;
                    value.ty = match value.ty {
                        Ty::Array(t) => *t,
                        Ty::Vector(_, kind) => Ty::Scalar(kind),
                        Ty::Matrix(_, r) => Ty::Vector(r, "float".into()),
                        _ => Ty::Unknown,
                    };
                    value.value = match (value.value, index.value) {
                        (Some(v), Some(i)) if i.len() == 1 => {
                            v.get(i[0] as usize).map(|x| vec![*x])
                        }
                        _ => None,
                    };
                    value.code = format!("{}[{}]", value.code, index.code);
                }
                Some("++" | "--") => {
                    value.code = format!("({}{})", value.code, self.tokens[self.pos]);
                    self.pos += 1;
                    value.value = None;
                }
                _ => break,
            }
        }
        Some(value)
    }
    fn call(&self, name: &str, mut args: Vec<Expr>) -> Expr {
        let constructor = Ty::parse(name);
        let mut ty = constructor.clone().unwrap_or(Ty::Unknown);
        let mut constant = None;
        if let Some(target) = &constructor {
            let all = args
                .iter()
                .map(|a| a.value.clone())
                .collect::<Option<Vec<_>>>();
            if let Some(values) = all {
                let flat = values.into_iter().flatten().collect::<Vec<_>>();
                let n = match target {
                    Ty::Scalar(_) => 1,
                    Ty::Vector(n, _) => *n as usize,
                    _ => 0,
                };
                if n > 0 {
                    constant = if flat.len() == 1 {
                        Some(vec![flat[0]; n])
                    } else if flat.len() >= n {
                        Some(flat[..n].to_vec())
                    } else {
                        None
                    };
                }
            }
        } else if [
            "texture",
            "textureLod",
            "textureGrad",
            "textureProj",
            "texelFetch",
            "texSample2DCompare",
        ]
        .contains(&name)
        {
            ty = Ty::Vector(4, "float".into());
        } else if ["dot", "length", "distance", "determinant"].contains(&name) {
            ty = Ty::Scalar("float".into());
        } else if name == "normalize" {
            ty = args.first().map(|a| a.ty.clone()).unwrap_or(Ty::Unknown);
            if let Some(v) = args.first().and_then(|a| a.value.as_ref()) {
                let length = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                if length > 0. {
                    constant = Some(v.iter().map(|x| x / length).collect());
                }
            }
        } else if [
            "pow",
            "min",
            "max",
            "mod",
            "atan",
            "step",
            "smoothstep",
            "mix",
            "clamp",
        ]
        .contains(&name)
        {
            ty = args.first().map(|a| a.ty.clone()).unwrap_or(Ty::Unknown);
            for arg in args.iter().skip(1) {
                ty = common(&ty, &arg.ty);
            }
            if !matches!(ty, Ty::Unknown) {
                for arg in &mut args {
                    *arg = coerce(arg.clone(), &ty);
                }
            }
        } else if [
            "abs",
            "sin",
            "cos",
            "tan",
            "asin",
            "acos",
            "exp",
            "exp2",
            "log",
            "log2",
            "sqrt",
            "inversesqrt",
            "floor",
            "ceil",
            "round",
            "trunc",
            "fract",
            "sign",
            "radians",
            "degrees",
            "dFdx",
            "dFdy",
            "fwidth",
        ]
        .contains(&name)
        {
            ty = args.first().map(|a| a.ty.clone()).unwrap_or(Ty::Unknown);
        } else if let Some(ret) = self.functions.get(name) {
            ty = ret.clone();
            if let Some(parameters) = self.parameters.get(name)
                && parameters.len() == args.len()
            {
                for (arg, (target, out)) in args.iter_mut().zip(parameters) {
                    if !*out {
                        *arg = coerce(arg.clone(), target);
                    }
                }
            }
        }
        let code = if name == "normalize"
            && let Some(values) = constant.as_ref()
        {
            format!(
                "{}({})",
                ty.glsl().unwrap_or_else(|| "vec3".into()),
                values
                    .iter()
                    .map(|x| format!("{x:?}"))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        } else {
            format!(
                "{name}({})",
                args.iter()
                    .map(|a| a.code.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            )
        };
        Expr {
            code,
            ty,
            value: constant,
        }
    }
}

pub(crate) fn normalize(tokens: &[String]) -> Vec<String> {
    let mut symbols = BTreeMap::new();
    let mut functions = BTreeMap::new();
    let mut parameters: BTreeMap<String, Vec<(Ty, bool)>> = BTreeMap::new();
    for i in 0..tokens.len().saturating_sub(2) {
        if let Some(mut ty) = Ty::parse(&tokens[i]) {
            let name = &tokens[i + 1];
            if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                || name.as_bytes().first().is_none_or(u8::is_ascii_digit)
            {
                continue;
            }
            if tokens[i + 2] == "(" {
                functions
                    .entry(name.clone())
                    .and_modify(|old| {
                        if *old != ty {
                            *old = Ty::Unknown
                        }
                    })
                    .or_insert(ty);
                let mut depth = 1;
                let mut end = i + 3;
                while end < tokens.len() && depth > 0 {
                    if tokens[end] == "(" {
                        depth += 1;
                    }
                    if tokens[end] == ")" {
                        depth -= 1;
                    }
                    if depth > 0 {
                        end += 1;
                    }
                }
                if end < tokens.len() {
                    let mut args = Vec::new();
                    for arg in tokens[i + 3..end].split(|s| s == ",") {
                        if arg.is_empty() {
                            continue;
                        }
                        let mut arg_ty =
                            arg.iter().find_map(|s| Ty::parse(s)).unwrap_or(Ty::Unknown);
                        if arg.iter().any(|s| s == "[") {
                            arg_ty = Ty::Array(Box::new(arg_ty));
                        }
                        args.push((arg_ty, arg.iter().any(|s| s == "out" || s == "inout")));
                    }
                    parameters
                        .entry(name.clone())
                        .and_modify(|old| {
                            if old.len() != args.len() {
                                old.clear();
                            } else {
                                for (a, b) in old.iter_mut().zip(&args) {
                                    if a.0 != b.0 {
                                        a.0 = Ty::Unknown;
                                    }
                                    a.1 |= b.1;
                                }
                            }
                        })
                        .or_insert(args);
                }
                continue;
            }
            if tokens[i + 2] == "[" {
                ty = Ty::Array(Box::new(ty));
            }
            symbols
                .entry(name.clone())
                .and_modify(|old| {
                    if *old != ty {
                        *old = Ty::Unknown
                    }
                })
                .or_insert(ty);
        }
    }
    symbols.insert("gl_Position".into(), Ty::Vector(4, "float".into()));
    symbols.insert("gl_FragColor".into(), Ty::Vector(4, "float".into()));
    symbols.insert("_wpe_FragColor".into(), Ty::Vector(4, "float".into()));
    let mut constants = BTreeMap::new();
    let mut output = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let assignment = ["=", "+=", "-=", "*=", "/="].contains(&tokens[i].as_str());
        let returning = tokens[i] == "return";
        if assignment || returning {
            let mut end = i + 1;
            let mut nesting = 0i32;
            while end < tokens.len() {
                let t = tokens[end].as_str();
                if nesting == 0 && t == ";" {
                    break;
                }
                if ["(", "["].contains(&t) {
                    nesting += 1;
                }
                if [")", "]"].contains(&t) {
                    nesting -= 1;
                    if nesting < 0 {
                        break;
                    }
                }
                if nesting == 0 && ["{", "}"].contains(&t) {
                    break;
                }
                end += 1;
            }
            if end < tokens.len() && tokens[end] == ";" {
                let mut parser = Parser {
                    tokens: &tokens[i + 1..end],
                    pos: 0,
                    symbols: &symbols,
                    functions: &functions,
                    parameters: &parameters,
                    constants: &constants,
                };
                if let Some(mut expr) = parser.parse(0)
                    && parser.pos == end - i - 1
                {
                    let mut target = Ty::Unknown;
                    let mut variable = None;
                    if assignment {
                        let start = (0..i)
                            .rev()
                            .find(|j| [";", "{", "}"].contains(&tokens[*j].as_str()))
                            .map(|j| j + 1)
                            .unwrap_or(0);
                        let lhs = &tokens[start..i];
                        for j in 0..lhs.len().saturating_sub(1) {
                            if let Some(ty) = Ty::parse(&lhs[j])
                                && j + 2 == lhs.len()
                            {
                                target = ty;
                                variable = Some(lhs[j + 1].clone());
                            }
                        }
                        if matches!(target, Ty::Unknown) {
                            for j in 0..lhs.len() {
                                if symbols.contains_key(&lhs[j]) && (j == 0 || lhs[j - 1] != ".") {
                                    let mut p = Parser {
                                        tokens: &lhs[j..],
                                        pos: 0,
                                        symbols: &symbols,
                                        functions: &functions,
                                        parameters: &parameters,
                                        constants: &constants,
                                    };
                                    if let Some(v) = p.parse(0)
                                        && p.pos == lhs.len() - j
                                    {
                                        target = v.ty;
                                        break;
                                    }
                                }
                            }
                        }
                        if !matches!(expr.ty, Ty::Matrix(..)) {
                            expr = coerce(expr, &target);
                        }
                        if let Some(name) = variable
                            && let Some(value) = &expr.value
                            && lhs.iter().any(|s| s == "const")
                        {
                            constants.insert(name, value.clone());
                        }
                    }
                    output.push(tokens[i].clone());
                    output.push(expr.code);
                    i = end;
                    continue;
                }
            }
        }
        output.push(tokens[i].clone());
        i += 1;
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hlsl_vector_truncation_and_broadcast() {
        let input="varying vec4 uv ; uniform vec2 scale ; void main ( ) { vec2 p = uv * scale ; float alpha = uv ; vec3 a = pow ( vec3 ( 0.5 ) , 2.0 ) ; }".split_whitespace().map(str::to_owned).collect::<Vec<_>>();
        let out = normalize(&input).join(" ");
        assert!(out.contains("vec2(uv)"));
        assert!(out.contains("(uv).x"));
        assert!(out.contains("vec3(2.0)"));
        let input = "vec2 x ; varying vec4 result ; void main ( ) { result . x = 1.0 ; }"
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert!(!normalize(&input).join(" ").contains("vec2(1.0)"));
    }
}
