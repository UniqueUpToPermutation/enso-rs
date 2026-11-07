use crate::ids::{ShaderId, TextureId};

/// Everything that can go wrong when creating or updating render resources.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("shader `{label}` failed to compile:\n{message}")]
    ShaderCompile { label: String, message: String },

    #[error("unknown texture {0:?} (never created, or destroyed)")]
    UnknownTexture(TextureId),

    #[error("unknown shader {0:?} (never created, or destroyed)")]
    UnknownShader(ShaderId),

    #[error("a material takes at most {max} textures, got {got}")]
    TooManyTextures { max: usize, got: usize },

    #[error("texture data is {got} bytes, expected {expected}")]
    TextureDataSize { expected: usize, got: usize },
}
