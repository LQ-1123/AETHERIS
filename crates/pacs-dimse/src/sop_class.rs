//! 接受哪些抽象语法(SOP Class)。
//!
//! # 为什么是硬编码清单
//!
//! `dicom-ul` 的协商只支持「显式清单」或「promiscuous(什么都收)」两种模式,
//! 没有谓词钩子。promiscuous 会连 Query/Retrieve 的表示上下文一起收下,
//! 而我们还没实现 C-FIND/C-MOVE —— 接受了再回错误码,比一开始就拒绝这个
//! 上下文要糟糕。所以列清单。
//!
//! 清单里写的是 UID 原文而不是 `uids::` 常量:UID 是 PS3.6 附录 A 里的规范
//! 标识,审阅时可以逐条对标准核。写错的风险由 [`tests`] 里的字典校验兜住 ——
//! 拼错的 UID 在字典里查不到,测试会失败,不会变成"某台设备连不进来"
//! 这种要等到现场才发现的缺口。

/// Verification SOP Class —— C-ECHO 用。
pub const VERIFICATION: &str = "1.2.840.10008.1.1";

/// C-FIND 支持的两种查询/检索信息模型(PS3.4 附录 C)。
///
/// 两者的差别只在**最浅层级**:Patient Root 从 PATIENT 层起,Study Root 从
/// STUDY 层起。Study Root 是主流查看器的默认选择 —— 影像科的检索入口是检查号
/// 和检查日期,而不是先选病人;而且不是所有影像都能可靠归到一个病人身上
/// (急诊的无名氏、外院带来的光盘)。两个都支持,让对端自己挑。
pub const FIND: &[&str] = &[
    "1.2.840.10008.5.1.4.1.2.1.1", // Patient Root Query/Retrieve - FIND
    "1.2.840.10008.5.1.4.1.2.2.1", // Study Root Query/Retrieve - FIND
];

/// Patient Root 的 C-FIND SOP Class,查询层级从 PATIENT 起。
pub const PATIENT_ROOT_FIND: &str = "1.2.840.10008.5.1.4.1.2.1.1";
/// Study Root 的 C-FIND SOP Class,查询层级从 STUDY 起。
pub const STUDY_ROOT_FIND: &str = "1.2.840.10008.5.1.4.1.2.2.1";

/// 接受的存储类 SOP Class。
///
/// 覆盖常见模态及其增强版/遗留转换版。设备用到清单外的 SOP Class 时,
/// 协商阶段就会拒绝该表示上下文,日志里能看到,补一行即可。
pub const STORAGE: &[&str] = &[
    // —— 投影 X 光 ——
    "1.2.840.10008.5.1.4.1.1.1",     // Computed Radiography Image Storage
    "1.2.840.10008.5.1.4.1.1.1.1",   // Digital X-Ray Image Storage - For Presentation
    "1.2.840.10008.5.1.4.1.1.1.1.1", // Digital X-Ray Image Storage - For Processing
    "1.2.840.10008.5.1.4.1.1.1.2",   // Digital Mammography X-Ray - For Presentation
    "1.2.840.10008.5.1.4.1.1.1.2.1", // Digital Mammography X-Ray - For Processing
    "1.2.840.10008.5.1.4.1.1.1.3",   // Digital Intra-Oral X-Ray - For Presentation
    "1.2.840.10008.5.1.4.1.1.1.3.1", // Digital Intra-Oral X-Ray - For Processing
    // —— CT ——
    "1.2.840.10008.5.1.4.1.1.2",   // CT Image Storage
    "1.2.840.10008.5.1.4.1.1.2.1", // Enhanced CT Image Storage
    "1.2.840.10008.5.1.4.1.1.2.2", // Legacy Converted Enhanced CT Image Storage
    // —— MR ——
    "1.2.840.10008.5.1.4.1.1.4",   // MR Image Storage
    "1.2.840.10008.5.1.4.1.1.4.1", // Enhanced MR Image Storage
    "1.2.840.10008.5.1.4.1.1.4.2", // MR Spectroscopy Storage
    "1.2.840.10008.5.1.4.1.1.4.3", // Enhanced MR Color Image Storage
    "1.2.840.10008.5.1.4.1.1.4.4", // Legacy Converted Enhanced MR Image Storage
    // —— 超声 ——
    "1.2.840.10008.5.1.4.1.1.3.1", // Ultrasound Multi-frame Image Storage
    "1.2.840.10008.5.1.4.1.1.6.1", // Ultrasound Image Storage
    "1.2.840.10008.5.1.4.1.1.6.2", // Enhanced US Volume Storage
    // —— 二次采集 ——
    "1.2.840.10008.5.1.4.1.1.7",   // Secondary Capture Image Storage
    "1.2.840.10008.5.1.4.1.1.7.1", // Multi-frame Single Bit SC Image Storage
    "1.2.840.10008.5.1.4.1.1.7.2", // Multi-frame Grayscale Byte SC Image Storage
    "1.2.840.10008.5.1.4.1.1.7.3", // Multi-frame Grayscale Word SC Image Storage
    "1.2.840.10008.5.1.4.1.1.7.4", // Multi-frame True Color SC Image Storage
    // —— 血管造影 / 透视 ——
    "1.2.840.10008.5.1.4.1.1.12.1", // X-Ray Angiographic Image Storage
    "1.2.840.10008.5.1.4.1.1.12.1.1", // Enhanced XA Image Storage
    "1.2.840.10008.5.1.4.1.1.12.2", // X-Ray Radiofluoroscopic Image Storage
    "1.2.840.10008.5.1.4.1.1.12.2.1", // Enhanced XRF Image Storage
    // —— 乳腺断层 ——
    "1.2.840.10008.5.1.4.1.1.13.1.3", // Breast Tomosynthesis Image Storage
    // —— 核医学 / PET ——
    "1.2.840.10008.5.1.4.1.1.20",  // Nuclear Medicine Image Storage
    "1.2.840.10008.5.1.4.1.1.128", // Positron Emission Tomography Image Storage
    "1.2.840.10008.5.1.4.1.1.130", // Enhanced PET Image Storage
    // —— 可见光 ——
    "1.2.840.10008.5.1.4.1.1.77.1.1", // VL Endoscopic Image Storage
    "1.2.840.10008.5.1.4.1.1.77.1.2", // VL Microscopic Image Storage
    "1.2.840.10008.5.1.4.1.1.77.1.4", // VL Photographic Image Storage
    "1.2.840.10008.5.1.4.1.1.77.1.6", // VL Whole Slide Microscopy Image Storage
    // —— 放疗 ——
    "1.2.840.10008.5.1.4.1.1.481.1", // RT Image Storage
    "1.2.840.10008.5.1.4.1.1.481.2", // RT Dose Storage
    "1.2.840.10008.5.1.4.1.1.481.3", // RT Structure Set Storage
    "1.2.840.10008.5.1.4.1.1.481.5", // RT Plan Storage
    // —— 非图像对象 ——
    "1.2.840.10008.5.1.4.1.1.11.1", // Grayscale Softcopy Presentation State Storage
    "1.2.840.10008.5.1.4.1.1.66",   // Raw Data Storage
    "1.2.840.10008.5.1.4.1.1.66.4", // Segmentation Storage
    "1.2.840.10008.5.1.4.1.1.88.11", // Basic Text SR Storage
    "1.2.840.10008.5.1.4.1.1.88.22", // Enhanced SR Storage
    "1.2.840.10008.5.1.4.1.1.88.33", // Comprehensive SR Storage
    "1.2.840.10008.5.1.4.1.1.88.59", // Key Object Selection Document Storage
    "1.2.840.10008.5.1.4.1.1.104.1", // Encapsulated PDF Storage
];

/// 接受的传输语法。
///
/// 顺序即优先级:发送方提议多个时,靠前的先被选中。
///
/// 未压缩格式排在最前 —— 我们能解也能编,后续转码、缩略图、帧提取都不用先
/// 解一层压缩。压缩格式排后面是「收得下」而不是「更想要」:JPEG 2000 能解码
/// 但没有编码器(dicom-pixeldata 0.10 的 openjp2 只有解码方向),
/// 一旦收进来就无法再转成别的压缩格式。
pub const TRANSFER_SYNTAXES: &[&str] = &[
    "1.2.840.10008.1.2.1",    // Explicit VR Little Endian
    "1.2.840.10008.1.2",      // Implicit VR Little Endian
    "1.2.840.10008.1.2.4.70", // JPEG Lossless, First-Order Prediction
    "1.2.840.10008.1.2.4.57", // JPEG Lossless, Non-Hierarchical
    "1.2.840.10008.1.2.4.50", // JPEG Baseline (Process 1)
    "1.2.840.10008.1.2.4.51", // JPEG Extended (Process 2 & 4)
    "1.2.840.10008.1.2.4.80", // JPEG-LS Lossless
    "1.2.840.10008.1.2.4.81", // JPEG-LS Lossy (Near-Lossless)
    "1.2.840.10008.1.2.4.90", // JPEG 2000 Image Compression (Lossless Only)
    "1.2.840.10008.1.2.4.91", // JPEG 2000 Image Compression
];

#[cfg(test)]
mod tests {
    use dicom::core::dictionary::UidDictionary;
    use dicom::dictionary_std::sop_class::StandardSopClassDictionary;
    use dicom::encoding::TransferSyntaxIndex;
    use dicom::transfer_syntax::TransferSyntaxRegistry;

    use super::*;

    /// 每个 UID 都要能在标准字典里查到,且确实是存储类。
    ///
    /// 这条测试是硬编码清单的安全网:UID 写错一位,字典就查不到,
    /// 这里当场失败 —— 而不是等某台设备接不进来才发现。
    #[test]
    fn every_storage_uid_resolves_to_a_storage_sop_class() {
        for uid in STORAGE {
            let entry = StandardSopClassDictionary
                .by_uid(uid)
                .unwrap_or_else(|| panic!("{uid} 在标准 SOP Class 字典里查不到,多半是写错了"));
            // 名字里带 Storage 即可,不能要求以它结尾 —— 「... Storage - For
            // Presentation」这类带后缀的也是正经存储类。
            assert!(
                entry.name.contains("Storage"),
                "{uid} 解析为 {:?},不像存储类 SOP Class",
                entry.name
            );
            // Storage Commitment 是承诺服务不是存储服务,收进来也没法存
            assert!(
                !entry.name.contains("Commitment"),
                "{uid} 是 {:?},不该出现在存储清单里",
                entry.name
            );
        }
    }

    #[test]
    fn verification_uid_is_correct() {
        let entry = StandardSopClassDictionary
            .by_uid(VERIFICATION)
            .expect("Verification 应在字典里");
        assert_eq!(entry.name, "Verification SOP Class");
    }

    #[test]
    fn no_duplicate_storage_uids() {
        let unique: std::collections::HashSet<_> = STORAGE.iter().collect();
        assert_eq!(unique.len(), STORAGE.len(), "清单里有重复项");
    }

    /// C-FIND 的两个信息模型 UID 同样对字典校验,防止写错一位。
    #[test]
    fn find_uids_resolve_to_query_retrieve_find_sop_classes() {
        for uid in FIND {
            let entry = StandardSopClassDictionary
                .by_uid(uid)
                .unwrap_or_else(|| panic!("{uid} 在标准 SOP Class 字典里查不到"));
            assert!(
                entry.name.contains("FIND"),
                "{uid} 解析为 {:?},不是 C-FIND 的信息模型",
                entry.name
            );
        }
        assert!(FIND.contains(&PATIENT_ROOT_FIND));
        assert!(FIND.contains(&STUDY_ROOT_FIND));
        assert_ne!(PATIENT_ROOT_FIND, STUDY_ROOT_FIND);
    }

    /// 提议的传输语法必须都是本地真能解码的,否则协商成功后反而读不了数据集。
    #[test]
    fn every_transfer_syntax_is_supported_locally() {
        for uid in TRANSFER_SYNTAXES {
            let ts = TransferSyntaxRegistry
                .get(uid)
                .unwrap_or_else(|| panic!("{uid} 不在传输语法注册表里"));
            assert!(
                ts.can_decode_all() || ts.can_decode_dataset(),
                "{uid} ({}) 本地无法解码,不该出现在协商清单里",
                ts.name()
            );
        }
    }

    /// 未压缩的两种要排在最前:它们是我们唯一能双向编解码的格式。
    #[test]
    fn uncompressed_syntaxes_have_priority() {
        assert_eq!(TRANSFER_SYNTAXES[0], "1.2.840.10008.1.2.1");
        assert_eq!(TRANSFER_SYNTAXES[1], "1.2.840.10008.1.2");
    }
}
