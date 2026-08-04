//! 领域模型:Patient/Study/Series/Instance 四层结构、UID 类型、元数据提取。
//!
//! 本 crate 不依赖数据库与网络,服务端与 Tauri 查看器共同复用 —— 查看器要能
//! 脱离服务端直接打开本地 `.dcm` 文件,靠的就是这一层加 `pacs-codec`。

pub mod attributes;
pub mod extract;
#[cfg(feature = "fixtures")]
pub mod fixture;
pub mod geometry;
pub mod model;
pub mod query;
pub mod spacing;
pub mod text;
pub mod uid;

pub use extract::{ExtractError, extract_metadata};
pub use geometry::{
    GeometryError, SliceInput, SortedSeries, Vec3, group_slices_by_orientation, sort_slices,
};
pub use model::{
    InstanceMeta, InstanceMetadata, PatientMeta, SeriesMeta, StudyMeta, normalize_person_name,
};
pub use query::{MatchKey, Query, QueryError, QueryLevel};
pub use spacing::{Confidence, Measurement, PixelSpacing, Spacing, distance, resolve};
pub use text::{
    TextNormalizationReport, normalize_dataset_text, normalize_file_text, normalized_text_element,
    utf8_text,
};
pub use uid::{Uid, UidError};

/// crate 版本,用于服务端启动日志与客户端 About 信息。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_set() {
        assert!(!VERSION.is_empty());
    }
}
