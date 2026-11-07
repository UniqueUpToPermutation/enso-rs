//! Named, typed shader parameters.
//!
//! A shader declares its knobs as a WGSL struct (`struct Material { .. }`
//! for per-material values, `struct Instance { .. }` for per-instance
//! values). When the shader is compiled, the renderer asks naga for that
//! struct's layout: each field's name, type and byte offset. Values set by
//! name from Rust are interpolated between ticks and packed into those
//! bytes before they are uploaded.
//!
//! Supported field types: `f32`, `vec2<f32>`, `vec3<f32>`, `vec4<f32>`.
//! Fields whose name starts with `_` are padding: allowed, never exposed.

use std::{
    collections::HashMap,
    marker::PhantomData,
    sync::{Mutex, OnceLock},
};

use glam::{Vec2, Vec3, Vec4};
use wgpu::naga;

// ---- names ----------------------------------------------------------------

/// An interned parameter name: cheap to copy, compare and hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct ParamName(u32);

fn interner() -> &'static Mutex<HashMap<String, u32>> {
    static NAMES: OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();
    NAMES.get_or_init(Default::default)
}

impl ParamName {
    pub(crate) fn intern(name: &str) -> Self {
        let mut names = interner().lock().unwrap();
        let next = names.len() as u32;
        Self(*names.entry(name.to_owned()).or_insert(next))
    }
}

// ---- values -----------------------------------------------------------------

/// The type of a parameter field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParamKind {
    F32,
    Vec2,
    Vec3,
    Vec4,
}

impl ParamKind {
    /// Number of `f32` components.
    pub fn components(self) -> usize {
        match self {
            Self::F32 => 1,
            Self::Vec2 => 2,
            Self::Vec3 => 3,
            Self::Vec4 => 4,
        }
    }
}

/// A parameter value: up to four floats plus their kind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ParamValue {
    pub kind: ParamKind,
    pub data: [f32; 4],
}

impl ParamValue {
    pub(crate) fn lerp(&self, other: &Self, t: f32) -> Self {
        Self {
            kind: other.kind,
            data: std::array::from_fn(|i| self.data[i] + (other.data[i] - self.data[i]) * t),
        }
    }
}

mod sealed {
    pub trait Sealed {}
}

/// Rust types that can be a parameter value: `f32`, `Vec2`, `Vec3`, `Vec4`.
pub trait ParamType: sealed::Sealed + Copy {
    const KIND: ParamKind;
    #[doc(hidden)]
    fn to_data(self) -> [f32; 4];
}

macro_rules! param_type {
    ($ty:ty, $kind:ident, |$v:ident| $data:expr) => {
        impl sealed::Sealed for $ty {}
        impl ParamType for $ty {
            const KIND: ParamKind = ParamKind::$kind;
            fn to_data(self) -> [f32; 4] {
                let $v = self;
                $data
            }
        }
    };
}
param_type!(f32, F32, |v| [v, 0.0, 0.0, 0.0]);
param_type!(Vec2, Vec2, |v| [v.x, v.y, 0.0, 0.0]);
param_type!(Vec3, Vec3, |v| [v.x, v.y, v.z, 0.0]);
param_type!(Vec4, Vec4, |v| v.to_array());

// ---- handles ------------------------------------------------------------------

/// A handle to a named shader parameter of type `T`.
///
/// Get a checked one from [`Renderer::material_param`] or
/// [`Renderer::instance_param`] (which verify the field exists with this
/// type), or an unchecked one with [`Param::new`]. Handles refer to the field
/// by *name*, so they keep working when a shader is hot-reloaded and its
/// struct changes; values for fields that no longer exist are ignored.
///
/// [`Renderer::material_param`]: crate::Renderer::material_param
/// [`Renderer::instance_param`]: crate::Renderer::instance_param
pub struct Param<T> {
    pub(crate) name: ParamName,
    _type: PhantomData<fn() -> T>,
}

// Manual impls: derive would require `T: Clone` etc.
impl<T> Clone for Param<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Param<T> {}
impl<T> std::fmt::Debug for Param<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Param").field(&self.name).finish()
    }
}

impl<T: ParamType> Param<T> {
    /// A handle without checking any shader. Prefer the checked
    /// constructors on [`Renderer`](crate::Renderer).
    pub fn new(name: &str) -> Self {
        Self { name: ParamName::intern(name), _type: PhantomData }
    }

    pub(crate) fn value(self, value: T) -> (ParamName, ParamValue) {
        (self.name, ParamValue { kind: T::KIND, data: value.to_data() })
    }
}

// ---- layouts --------------------------------------------------------------------

/// One field of a parameter struct, for listing (e.g. to build a GUI).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamInfo {
    pub name: String,
    pub kind: ParamKind,
}

/// The byte layout of a shader's parameter struct, from naga.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParamLayout {
    /// Struct size in bytes, including trailing padding (= array stride).
    pub size: u32,
    fields: Vec<Field>,
    infos: Vec<ParamInfo>,
}

#[derive(Debug, Clone, PartialEq)]
struct Field {
    name: ParamName,
    kind: ParamKind,
    offset: u32,
}

impl ParamLayout {
    pub(crate) fn infos(&self) -> &[ParamInfo] {
        &self.infos
    }

    pub(crate) fn kind_of(&self, name: ParamName) -> Option<ParamKind> {
        self.field(name).map(|f| f.kind)
    }

    fn field(&self, name: ParamName) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// Write one value into a block laid out by `self`. Unknown names and
    /// mismatched kinds are ignored (e.g. after a hot reload changed the struct).
    pub(crate) fn write(&self, block: &mut [u8], name: ParamName, value: &ParamValue) {
        let Some(field) = self.field(name) else { return };
        if field.kind != value.kind {
            return;
        }
        let bytes: &[u8] = bytemuck::cast_slice(&value.data[..field.kind.components()]);
        let start = field.offset as usize;
        block[start..start + bytes.len()].copy_from_slice(bytes);
    }

    /// Append one block to `out`: zeros, then `curr`'s values, each
    /// interpolated with the same-named value in `prev` when there is one.
    pub(crate) fn pack(
        &self,
        prev: &[(ParamName, ParamValue)],
        curr: &[(ParamName, ParamValue)],
        alpha: f32,
        out: &mut Vec<u8>,
    ) {
        let start = out.len();
        out.resize(start + self.size as usize, 0);
        let block = &mut out[start..];
        for (name, value) in curr {
            let value = match prev.iter().find(|(n, v)| n == name && v.kind == value.kind) {
                Some((_, prev)) => prev.lerp(value, alpha),
                None => *value,
            };
            self.write(block, *name, &value);
        }
    }

    /// Read the layout of the struct bound at `@group(group) @binding(binding)`
    /// (or of the element type, if it is bound as an array).
    pub(crate) fn reflect(source: &str, group: u32, binding: u32) -> Result<Self, String> {
        let module = naga::front::wgsl::parse_str(source).map_err(|e| e.emit_to_string(source))?;

        let (_, var) = module
            .global_variables
            .iter()
            .find(|(_, v)| v.binding.as_ref().is_some_and(|b| b.group == group && b.binding == binding))
            .ok_or_else(|| format!("no variable at @group({group}) @binding({binding})"))?;

        let mut ty = &module.types[var.ty];
        if let naga::TypeInner::Array { base, .. } = ty.inner {
            ty = &module.types[base];
        }
        let struct_name = ty.name.clone().unwrap_or_default();
        let naga::TypeInner::Struct { members, span } = &ty.inner else {
            return Err(format!("`{struct_name}` must be a struct"));
        };

        let mut fields = Vec::new();
        let mut infos = Vec::new();
        for member in members {
            let name = member.name.clone().unwrap_or_default();
            if name.starts_with('_') {
                continue; // padding
            }
            let kind = match module.types[member.ty].inner {
                naga::TypeInner::Scalar(s) if s == naga::Scalar::F32 => ParamKind::F32,
                naga::TypeInner::Vector { size, scalar } if scalar == naga::Scalar::F32 => match size {
                    naga::VectorSize::Bi => ParamKind::Vec2,
                    naga::VectorSize::Tri => ParamKind::Vec3,
                    naga::VectorSize::Quad => ParamKind::Vec4,
                },
                _ => {
                    return Err(format!(
                        "field `{struct_name}.{name}` must be f32, vec2<f32>, vec3<f32> or vec4<f32> \
                         (prefix the name with `_` to use it as padding)"
                    ));
                }
            };
            fields.push(Field { name: ParamName::intern(&name), kind, offset: member.offset });
            infos.push(ParamInfo { name, kind });
        }
        Ok(Self { size: *span, fields, infos })
    }
}

/// Whether `source` declares `struct <name>` (ignoring `//` comments).
pub(crate) fn declares_struct(source: &str, name: &str) -> bool {
    source.lines().any(|line| {
        let code = line.split("//").next().unwrap_or("");
        let mut words = code.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| !w.is_empty());
        while let Some(word) = words.next() {
            if word == "struct" && words.next() == Some(name) {
                return true;
            }
        }
        false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = r#"
        struct Material {
            base_color: vec3<f32>,
            glow_color: vec3<f32>,
            pulse_speed: f32,
        };
        @group(1) @binding(0) var<uniform> material: Material;

        struct Instance { velocity: f32, _pad: u32, age: f32 };
        @group(2) @binding(0) var<storage, read> instances: array<Instance>;
    "#;

    #[test]
    fn reflects_uniform_layout_rules() {
        let layout = ParamLayout::reflect(SOURCE, 1, 0).unwrap();
        // vec3 aligns to 16: base_color @0, glow_color @16, pulse_speed fits after it @28.
        let offsets: Vec<u32> = layout.fields.iter().map(|f| f.offset).collect();
        assert_eq!(offsets, [0, 16, 28]);
        assert_eq!(layout.size, 32);
        assert_eq!(layout.infos()[2], ParamInfo { name: "pulse_speed".into(), kind: ParamKind::F32 });
    }

    #[test]
    fn reflects_array_element_and_skips_padding() {
        let layout = ParamLayout::reflect(SOURCE, 2, 0).unwrap();
        let names: Vec<&str> = layout.infos().iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["velocity", "age"]);
        assert_eq!(layout.size, 12);
    }

    #[test]
    fn rejects_unsupported_field_types() {
        let source = "struct Material { count: u32 }; @group(1) @binding(0) var<uniform> m: Material;";
        let err = ParamLayout::reflect(source, 1, 0).unwrap_err();
        assert!(err.contains("Material.count"), "{err}");
    }

    #[test]
    fn packs_and_interpolates_by_name() {
        let layout = ParamLayout::reflect(SOURCE, 2, 0).unwrap();
        let velocity = Param::<f32>::new("velocity");
        let age = Param::<f32>::new("age");
        let prev = [velocity.value(0.0), age.value(1.0)];
        let curr = [velocity.value(1.0), age.value(3.0)];

        let mut out = Vec::new();
        layout.pack(&prev, &curr, 0.5, &mut out);
        let floats: &[f32] = bytemuck::cast_slice(&out);
        assert_eq!(floats[0], 0.5); // velocity @0
        assert_eq!(floats[2], 2.0); // age @8 (after the 4-byte padding field)
    }

    #[test]
    fn finds_struct_declarations_outside_comments() {
        assert!(declares_struct("struct Instance { a: f32 }", "Instance"));
        assert!(declares_struct("  struct   Material{", "Material"));
        assert!(!declares_struct("// struct Material { }", "Material"));
        assert!(!declares_struct("struct MaterialX { }", "Material"));
    }
}
