//! 存储布局:影像文件在磁盘上放哪儿。

use pacs_core::Uid;
use sha2::{Digest, Sha256};

/// 一个实例的三级 UID 定位。
///
/// 三个字段都是 `&Uid`,用结构体而不是三个位置参数,免得调用处顺序写反 ——
/// 类型系统挡不住这种错,但字段名可以。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceKey<'a> {
    pub study: &'a Uid,
    pub series: &'a Uid,
    pub sop: &'a Uid,
}

/// 相对存储根的路径:`<h0>/<h1>/<StudyUID>/<SeriesUID>/<SOPUID>.dcm`。
///
/// 前两级是 StudyInstanceUID 的 SHA-256 前两个字节,共 65536 个桶。
///
/// - **分片是为了控制单目录 fanout**。几十万个子目录挤在一层会拖垮文件系统的
///   目录查找,几乎所有文件系统都是这样。
/// - **按 study 分片而不是按实例**,保住了 study/series 的局部性:WADO 拉整个
///   series 是顺序读同一个目录。
/// - 用哈希而不是 UID 前缀,是因为同一台设备产出的 UID 前缀高度相同,
///   直接切前缀会让绝大多数 study 落进同一个桶,等于没分片。
///
/// 路径分量全部来自校验过的 [`Uid`],不含分隔符也不会是 `.`/`..`,
/// 拼接不会越出存储根。
pub fn relative_path(key: InstanceKey<'_>) -> String {
    let digest = Sha256::digest(key.study.as_str().as_bytes());
    format!(
        "{:02x}/{:02x}/{}/{}/{}.dcm",
        digest[0], digest[1], key.study, key.series, key.sop
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uid(s: &str) -> Uid {
        Uid::parse(s).expect("测试 UID 应合法")
    }

    #[test]
    fn path_has_two_shard_levels_then_uid_hierarchy() {
        let (study, series, sop) = (uid("1.2.3"), uid("1.2.3.4"), uid("1.2.3.4.5"));
        let path = relative_path(InstanceKey {
            study: &study,
            series: &series,
            sop: &sop,
        });

        let parts: Vec<&str> = path.split('/').collect();
        assert_eq!(parts.len(), 5, "两级分片 + study + series + 文件名");
        assert_eq!(parts[0].len(), 2);
        assert_eq!(parts[1].len(), 2);
        assert_eq!(parts[2], "1.2.3");
        assert_eq!(parts[3], "1.2.3.4");
        assert_eq!(parts[4], "1.2.3.4.5.dcm");
    }

    #[test]
    fn same_study_shares_a_directory() {
        let study = uid("1.2.3");
        let series = uid("1.2.3.4");
        let (a, b) = (uid("1.2.3.4.5"), uid("1.2.3.4.6"));

        let path_a = relative_path(InstanceKey {
            study: &study,
            series: &series,
            sop: &a,
        });
        let path_b = relative_path(InstanceKey {
            study: &study,
            series: &series,
            sop: &b,
        });

        let dir = |p: &str| p.rsplit_once('/').expect("应有目录").0.to_owned();
        assert_eq!(
            dir(&path_a),
            dir(&path_b),
            "同序列的实例应在同一目录,保住顺序读局部性"
        );
    }

    #[test]
    fn shards_spread_across_buckets() {
        // UID 前缀相同(同一台设备)时也要散开 —— 这正是用哈希而不是切前缀的理由
        let series = uid("1");
        let sop = uid("1");
        let buckets: std::collections::HashSet<String> = (0..200)
            .map(|i| {
                let study = uid(&format!("1.2.840.113619.2.55.3.{i}"));
                relative_path(InstanceKey {
                    study: &study,
                    series: &series,
                    sop: &sop,
                })
                .split('/')
                .take(2)
                .collect::<Vec<_>>()
                .join("/")
            })
            .collect();
        assert!(
            buckets.len() > 190,
            "200 个相似 UID 应散进接近 200 个桶,实际 {}",
            buckets.len()
        );
    }

    #[test]
    fn generated_paths_stay_inside_the_root() {
        let (study, series, sop) = (uid("1.2.3"), uid("1.2.3.4"), uid("1.2.3.4.5"));
        let path = relative_path(InstanceKey {
            study: &study,
            series: &series,
            sop: &sop,
        });
        let joined = std::path::Path::new("/srv/pacs").join(&path);
        assert!(joined.starts_with("/srv/pacs"));
        assert!(
            !joined
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir)),
            "不该出现 `..`"
        );
    }
}
