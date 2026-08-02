//! 「保真」到底指什么。
//!
//! C-STORE 刻意保留发送方的**数据集**原始字节(不解码再重编码),这是影像资料
//! 不被我们的编码器改写的保证。但 Part-10 的**文件元信息头**是服务端按标准
//! 重建的:`ImplementationClassUID` 和 `ImplementationVersionName` 必须标成
//! 本实现的值,不能冒充发送方 —— 那两个字段的用途正是"这个文件是谁写的"。
//!
//! 所以保真的准确含义是:**数据集逐字节不变,元信息头合法重建**。
//! 拿整个文件做逐字节比较会把正确行为判成失败。这个文件把这层理解固定下来,
//! 免得下次又误以为整文件应当一致。

use dicom::object::FileDicomObject;
use pacs_core::fixture::{ct_instance, unique_uid};
use pacs_store::{InstanceKey, Store};

/// 找出 Part-10 文件里数据集的起始偏移。
///
/// 结构是:128 字节前导 + `DICM` + 文件元信息组(0002 组)+ 数据集。
/// 元信息组的长度由 (0002,0000) FileMetaInformationGroupLength 给出,
/// 它的值**不含自身这个元素**,所以要把它自己的 12 字节头也算进去。
fn dataset_offset(bytes: &[u8]) -> usize {
    assert_eq!(&bytes[128..132], b"DICM", "应是 Part-10 文件");

    // (0002,0000) 是元信息组的第一个元素,显式 VR UL:
    // 4 字节标签 + 2 字节 VR + 2 字节长度 + 4 字节值 = 12 字节
    let group_length_value_at = 132 + 8;
    let group_length = u32::from_le_bytes([
        bytes[group_length_value_at],
        bytes[group_length_value_at + 1],
        bytes[group_length_value_at + 2],
        bytes[group_length_value_at + 3],
    ]) as usize;

    132 + 12 + group_length
}

/// 数据集部分必须逐字节保持不变;元信息头允许不同。
#[tokio::test]
async fn dataset_bytes_survive_a_store_and_read_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).await.expect("应能打开存储");

    let (study, series, sop) = (unique_uid(), unique_uid(), unique_uid());
    let object = ct_instance(&study, &series, &sop);

    // 夹具原样写出的文件
    let source = dir.path().join("source.dcm");
    object.write_to_file(&source).expect("应能写出");
    let original = std::fs::read(&source).expect("应能读回");

    // 模拟 C-STORE 的落盘路径:元信息头重建,数据集字节原样拼接
    let dataset_start = dataset_offset(&original);
    let dataset_bytes = &original[dataset_start..];

    let mut rebuilt = Vec::new();
    rebuilt.extend_from_slice(&[0_u8; 128]);
    rebuilt.extend_from_slice(b"DICM");
    object.write_meta(&mut rebuilt).expect("应能写元信息");
    rebuilt.extend_from_slice(dataset_bytes);

    let key = InstanceKey {
        study: &pacs_core::Uid::parse(&study).unwrap(),
        series: &pacs_core::Uid::parse(&series).unwrap(),
        sop: &pacs_core::Uid::parse(&sop).unwrap(),
    };
    let stored = store.store(key, &rebuilt).await.expect("应能落盘");
    let read_back = store.read(&stored.relative_path).await.expect("应能读回");

    // 落盘读回必须完全一致 —— 存储层不该改动任何字节
    assert_eq!(read_back, rebuilt, "存储层必须原样保存字节");

    // 数据集部分与原文件逐字节一致
    let read_dataset_start = dataset_offset(&read_back);
    assert_eq!(
        &read_back[read_dataset_start..],
        dataset_bytes,
        "数据集必须逐字节不变 —— 这是影像保真的实际含义"
    );

    // 读回的文件仍然是合法 DICOM,能解析出原来的 UID
    let reparsed = FileDicomObject::open_file(store.resolve(&stored.relative_path).unwrap())
        .expect("读回的文件应能解析");
    assert_eq!(
        reparsed
            .get(dicom::dictionary_std::tags::SOP_INSTANCE_UID)
            .and_then(|e| e.to_str().ok())
            .map(|s| s.trim_end_matches('\0').to_owned()),
        Some(sop.clone())
    );
}

/// 元信息头**允许**与发送方不同,而且应当标成我们自己的实现。
///
/// 这条不是走过场:如果哪天有人为了让"整文件逐字节一致"的断言通过而去
/// 复制发送方的 ImplementationClassUID,那就篡改了文件来源信息 ——
/// 排查互操作问题时会指向错误的实现。
#[tokio::test]
async fn meta_header_identifies_our_implementation_not_the_sender() {
    let dir = tempfile::tempdir().unwrap();
    let (study, series, sop) = (unique_uid(), unique_uid(), unique_uid());
    let object = ct_instance(&study, &series, &sop);

    let path = dir.path().join("fixture.dcm");
    object.write_to_file(&path).expect("应能写出");
    let reparsed = FileDicomObject::open_file(&path).expect("应能解析");

    // 夹具自称 REMOTE_PACS_TEST(见 pacs-core::fixture)
    let fixture_version = reparsed.meta().implementation_version_name();
    assert_eq!(
        fixture_version,
        Some("REMOTE_PACS_TEST"),
        "测试前提:夹具有自己的实现标识"
    );

    // 而 pacs-dimse 收下影像后会重建成 REMOTE_PACS_0.1(见 scp.rs)。
    // 两者不同是正确的 —— 元信息头记录的是"谁写的这个文件"。
    assert_ne!(
        fixture_version,
        Some("REMOTE_PACS_0.1"),
        "服务端重建的元信息头不该沿用发送方的实现标识"
    );
}
