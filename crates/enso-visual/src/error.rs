use crate::{
    ids::{MaterialId, MeshShaderId, SkinnedShaderId, SkyShaderId, TextShaderId, TextureId},
    params::ParamKind,
};

/// Everything that can go wrong when creating or updating render resources.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("shader `{label}` failed to compile:\n{message}")]
    ShaderCompile { label: String, message: String },

    #[error("unknown texture {0:?} (never created, or destroyed)")]
    UnknownTexture(TextureId),

    #[error("unknown mesh shader {0:?} (never created, or destroyed)")]
    UnknownShader(MeshShaderId),

    #[error("unknown skinned shader {0:?} (never created, or destroyed)")]
    UnknownSkinnedShader(SkinnedShaderId),

    #[error("unknown sky shader {0:?} (never created, or destroyed)")]
    UnknownSkyShader(SkyShaderId),

    #[error("unknown text shader {0:?} (never created, or destroyed)")]
    UnknownTextShader(TextShaderId),

    #[error("invalid font: {0}")]
    InvalidFont(String),

    #[error("a material takes at most {max} textures, got {got}")]
    TooManyTextures { max: usize, got: usize },

    #[error("texture data is {got} bytes, expected {expected}")]
    TextureDataSize { expected: usize, got: usize },

    #[error("invalid skeleton: {0}")]
    InvalidSkeleton(String),

    #[error("shader `{label}` has an unsupported parameter struct: {message}")]
    ShaderParams { label: String, message: String },

    #[error("unknown material {0:?} (never created, or destroyed)")]
    UnknownMaterial(MaterialId),

    #[error("no parameter `{name}`; available: {available}")]
    UnknownParam { name: String, available: String },

    #[error("parameter `{name}` is {found:?}, not {expected:?}")]
    ParamKindMismatch { name: String, expected: ParamKind, found: ParamKind },

    #[error("failed to read pixels back from the GPU: {0}")]
    Readback(String),
}
