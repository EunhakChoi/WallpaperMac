//! Preserve the IR array stride in Naga 27's generated Metal buffer types.
//!
//! Naga's GLSL frontend retains std140 scalar/vec2 array strides. Its MSL writer
//! emits those arrays as ordinary C++ arrays and ignores the stride. Padding
//! each element, with an address-space-qualified subscript returning the actual
//! value, keeps the validated IR, reflected host packing and shader arithmetic
//! intact. Natural function-argument arrays keep their ordinary storage.
use crate::{Error, Result};

pub(crate) fn preserve_array_strides(
    module: &naga::Module,
    layouter: &naga::proc::Layouter,
    source: &str,
) -> Result<(String, usize)> {
    let layouts = module
        .types
        .iter()
        .filter_map(|(_, ty)| match ty.inner {
            naga::TypeInner::Array {
                base,
                size: naga::ArraySize::Constant(count),
                stride,
            } => Some((count.get(), stride, layouter[base].to_stride())),
            _ => None,
        })
        .collect::<Vec<_>>();
    let lines = source.lines().collect::<Vec<_>>();
    let mut output = String::with_capacity(source.len());
    let mut cursor = 0;
    let mut array = 0;
    let mut padded = 0;
    while cursor < lines.len() {
        // This is the exact synthetic array-struct form emitted by the pinned
        // Naga MSL backend, in type-arena order. Fail if that contract changes.
        if lines[cursor].starts_with("struct ")
            && let Some(declaration) = lines.get(cursor + 1)
            && let Some((element, count)) = declaration.trim().split_once(" inner[")
            && lines.get(cursor + 2) == Some(&"};")
        {
            let count = count
                .strip_suffix("];")
                .and_then(|s| s.parse::<u32>().ok())
                .ok_or_else(|| Error::unsupported(0, "unexpected Metal array declaration"))?;
            let Some(&(expected, stride, natural)) = layouts.get(array) else {
                return Err(Error::unsupported(0, "unexpected Metal array type"));
            };
            if count != expected || stride < natural {
                return Err(Error::unsupported(
                    0,
                    "Metal array count/stride differs from validated shader IR",
                ));
            }
            array += 1;
            if stride > natural {
                let name = lines[cursor]
                    .strip_prefix("struct ")
                    .and_then(|s| s.strip_suffix(" {"))
                    .ok_or_else(|| Error::unsupported(0, "unexpected Metal array type name"))?;
                output.push_str(lines[cursor]);
                output.push('\n');
                output.push_str(&format!(
                    "    struct WpeStridedElement {{\n        {element} value;\n        char padding[{}];\n    }};\n    struct WpeStridedStorage {{\n        WpeStridedElement elements[{count}];\n",
                    stride - natural,
                ));
                output.push_str(&format!(
                    "        const constant {element}& operator[](unsigned int i) const constant {{ return elements[i].value; }}\n"
                ));
                for space in ["device", "thread", "threadgroup"] {
                    output.push_str(&format!(
                        "        {space} {element}& operator[](unsigned int i) {space} {{ return elements[i].value; }}\n        const {space} {element}& operator[](unsigned int i) const {space} {{ return elements[i].value; }}\n"
                    ));
                }
                output.push_str("    } inner;\n};\n");
                output.push_str(&format!(
                    "static_assert(sizeof({name}::WpeStridedElement) == {stride}, \"Metal array element stride\");\nstatic_assert(sizeof({name}) == {}, \"Metal array storage size\");\n",
                    u64::from(count) * u64::from(stride),
                ));
                cursor += 3;
                padded += 1;
                continue;
            }
        }
        output.push_str(lines[cursor]);
        output.push('\n');
        cursor += 1;
    }
    if array != layouts.len() {
        return Err(Error::unsupported(
            0,
            "Metal array declarations do not match the validated shader IR",
        ));
    }
    Ok((output, padded))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_backend_array_declarations_are_rejected() {
        let module = naga::front::glsl::Frontend::default()
            .parse(
                &naga::front::glsl::Options::from(naga::ShaderStage::Fragment),
                "#version 450\nlayout(std140,binding=0) uniform Params { float values[2]; };layout(location=0) out vec4 color;void main(){color=vec4(values[1]);}",
            )
            .unwrap();
        let mut layout = naga::proc::Layouter::default();
        layout.update(module.to_ctx()).unwrap();
        assert!(preserve_array_strides(&module, &layout, "// no array declarations\n").is_err());
    }
}
