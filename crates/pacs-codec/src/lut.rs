//! 查找表:把整条显示管线预先算成一张表。
//!
//! # 为什么需要它
//!
//! 窗宽窗位是**交互式**的 —— 医生拖鼠标时每帧都在变。两种做法各有问题:
//!
//! - **每次变动都让 Rust 重新渲染整帧**:512×512 要走 26 万次管线,
//!   还要把结果搬过 IPC。拖动时每秒几十次,IPC 成为瓶颈。
//! - **在 TypeScript 里重写窗宽公式**:交互变快了,但 LINEAR 的 `w-1`/`c-0.5`
//!   偏移、SIGMOID、MONOCHROME1 反转全要再写一遍。两套实现迟早分叉,
//!   而分叉的症状是"网页上的灰度和服务端渲染的不一样"—— 很难定位。
//!
//! LUT 兼顾两者:管线只在 Rust 实现一次([`crate::display`]),窗宽窗位变动时
//! 只重算这张表(16 位是 65536 次,亚毫秒),JS 侧的内层循环退化成一次数组索引。
//!
//! # 索引是原始位模式,不是解释后的值
//!
//! 表按**帧缓冲里的原始无符号整数**索引。有符号影像(CT 常见
//! `PixelRepresentation = 1`)的 `-1024` 在缓冲里是 `0xFC00`,
//! JS 从 `Uint16Array` 读出来就是 `64512` —— 直接拿它查表即可,
//! 不必在 JS 里做补码转换。
//!
//! 这个选择是为了 JS 侧的性能:补码转换要分支,而分支在 26 万次的循环里
//! 会毁掉分支预测。表把这件事吸收掉了。

use crate::display::{Pipeline, Window};

/// 一张查找表。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrayLut {
    /// `table[原始位模式] = 8 位灰度`。
    pub table: Vec<u8>,
    /// 表的位宽,等于 `BitsAllocated`。表长是 `1 << bits`。
    pub bits: u16,
}

/// 支持的最大位宽。
///
/// 16 位覆盖全部常规影像(CT/MR/CR/DX 都是 12-16 位存 16 位)。
/// 再大的表会占过多内存(18 位就是 256 KiB),而真正需要更高位深的
/// 浮点像素(`FloatPixelData`)本来就不走整数 LUT 这条路。
pub const MAX_LUT_BITS: u16 = 16;

impl GrayLut {
    /// 按管线和指定窗生成查找表。
    ///
    /// `bits_allocated` 取自 (0028,0100),决定表长。传 `None` 时按管线里的
    /// `bits_stored` 向上取到 8 或 16 —— 帧缓冲的元素宽度是 `BitsAllocated`
    /// 而不是 `BitsStored`(12 位存储也占 16 位),索引范围必须按前者。
    pub fn build(pipeline: &Pipeline, window: Option<&Window>, bits_allocated: Option<u16>) -> Self {
        let bits = bits_allocated
            .unwrap_or(if pipeline.bits_stored <= 8 { 8 } else { 16 })
            .clamp(1, MAX_LUT_BITS);
        let len = 1_usize << bits;

        let table = (0..len)
            .map(|raw| {
                let stored = interpret(raw as u32, bits, pipeline.signed);
                pipeline.apply(stored, window)
            })
            .collect();

        Self { table, bits }
    }

    /// 查表。越界返回 0 而不是 panic —— 一个坏像素不该让整帧渲染失败。
    pub fn lookup(&self, raw: u16) -> u8 {
        self.table.get(usize::from(raw)).copied().unwrap_or(0)
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    /// 直接给自定义协议用的字节。
    ///
    /// 表本身就是 `Vec<u8>`,零拷贝转出去。
    pub fn as_bytes(&self) -> &[u8] {
        &self.table
    }
}

/// 把原始位模式解释成存储值。
///
/// 有符号时按二进制补码:最高位为 1 的值减去 `2^bits`。
/// 这一步是 LUT 存在的意义之一 —— JS 侧不必再做。
fn interpret(raw: u32, bits: u16, signed: bool) -> f64 {
    if !signed {
        return f64::from(raw);
    }
    let sign_bit = 1_u32 << (bits - 1);
    if raw & sign_bit == 0 {
        f64::from(raw)
    } else {
        // 补码:0xFC00(16 位)→ 64512 - 65536 = -1024
        f64::from(raw) - 2_f64.powi(i32::from(bits))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::{ModalityLut, Photometric, VoiFunction};

    fn ct_pipeline(signed: bool) -> Pipeline {
        Pipeline {
            // CT 的典型 Rescale:存储值 - 1024 = HU
            modality_lut: ModalityLut {
                slope: 1.0,
                intercept: -1024.0,
                unit: Some("HU"),
            },
            windows: vec![Window {
                center: -600.0,
                width: 1500.0,
                explanation: Some("LUNG".to_owned()),
                function: VoiFunction::Linear,
            }],
            photometric: Photometric::Monochrome2,
            bits_stored: 16,
            signed,
        }
    }

    /// 表长由 BitsAllocated 决定,不是 BitsStored。
    ///
    /// 12 位存储的 CT 在缓冲里占 16 位,索引能达到 65535 ——
    /// 按 BitsStored 建 4096 长的表会让高位值查表越界。
    #[test]
    fn table_length_follows_bits_allocated() {
        let mut pipeline = ct_pipeline(false);
        pipeline.bits_stored = 12;

        let lut = GrayLut::build(&pipeline, None, Some(16));
        assert_eq!(lut.len(), 65536, "12 位存储但 16 位分配,表长必须是 65536");
        assert_eq!(lut.bits, 16);

        // 8 位影像的表短得多
        let eight = GrayLut::build(&pipeline, None, Some(8));
        assert_eq!(eight.len(), 256);
    }

    /// 表的每一项都要与直接走管线的结果一致 —— 这是 LUT 的正确性根基。
    #[test]
    fn every_entry_matches_the_pipeline() {
        let pipeline = ct_pipeline(false);
        let lut = GrayLut::build(&pipeline, None, Some(16));

        // 全表逐项比对代价太大,抽样覆盖各区间的边界
        for raw in [0_u16, 1, 100, 424, 1024, 2048, 4095, 4096, 32767, 32768, 65535] {
            let direct = pipeline.apply(f64::from(raw), None);
            assert_eq!(
                lut.lookup(raw),
                direct,
                "raw={raw} 时表值 {} 与管线结果 {direct} 不一致",
                lut.lookup(raw)
            );
        }
    }

    /// 有符号影像:表按原始位模式索引,补码转换由表吸收。
    ///
    /// 这是 JS 侧性能的关键 —— 它从 Uint16Array 读到 0xFC00 就直接查表,
    /// 不必判断符号。
    #[test]
    fn signed_values_are_indexed_by_their_raw_bit_pattern() {
        let pipeline = ct_pipeline(true);
        let lut = GrayLut::build(&pipeline, None, Some(16));

        // -1024 的 16 位补码是 0xFC00 = 64512
        let raw_minus_1024 = 0xFC00_u16;
        // 管线拿到的应该是 -1024,经 Rescale 得 -2048 HU
        let expected = pipeline.apply(-1024.0, None);
        assert_eq!(
            lut.lookup(raw_minus_1024),
            expected,
            "0xFC00 应被解释成 -1024,而不是 64512"
        );

        // 对照:同一个位模式在无符号管线下解释成 64512,结果不同
        let unsigned_lut = GrayLut::build(&ct_pipeline(false), None, Some(16));
        assert_ne!(
            lut.lookup(raw_minus_1024),
            unsigned_lut.lookup(raw_minus_1024),
            "有符号与无符号对同一位模式的解释必须不同,否则 signed 标志没生效"
        );

        // 正数部分两者一致
        assert_eq!(lut.lookup(100), unsigned_lut.lookup(100));
    }

    #[test]
    fn twos_complement_interpretation_is_correct() {
        // 16 位
        assert_eq!(interpret(0, 16, true), 0.0);
        assert_eq!(interpret(1, 16, true), 1.0);
        assert_eq!(interpret(32767, 16, true), 32767.0);
        assert_eq!(interpret(32768, 16, true), -32768.0);
        assert_eq!(interpret(0xFC00, 16, true), -1024.0);
        assert_eq!(interpret(65535, 16, true), -1.0);

        // 8 位
        assert_eq!(interpret(127, 8, true), 127.0);
        assert_eq!(interpret(128, 8, true), -128.0);
        assert_eq!(interpret(255, 8, true), -1.0);

        // 无符号时原样
        assert_eq!(interpret(65535, 16, false), 65535.0);
        assert_eq!(interpret(32768, 16, false), 32768.0);
    }

    /// 换窗只需重建表,管线不变 —— 这正是交互式窗宽窗位的实现方式。
    #[test]
    fn a_different_window_yields_a_different_table() {
        let pipeline = ct_pipeline(false);
        let lung = GrayLut::build(&pipeline, None, Some(16));

        let mediastinum = Window {
            center: 50.0,
            width: 350.0,
            explanation: Some("MEDIASTINUM".to_owned()),
            function: VoiFunction::Linear,
        };
        let soft = GrayLut::build(&pipeline, Some(&mediastinum), Some(16));

        assert_eq!(lung.len(), soft.len(), "换窗不改变表长");
        assert_ne!(lung.table, soft.table, "不同窗必须产生不同的表");

        // 纵隔窗更窄,同一个存储值的灰度差异应当可见
        let raw = 1074_u16; // HU = 50,正好是纵隔窗心
        assert_ne!(lung.lookup(raw), soft.lookup(raw));
    }

    /// MONOCHROME1 的表应当是 MONOCHROME2 的镜像。
    #[test]
    fn monochrome1_produces_an_inverted_table() {
        let mut pipeline = ct_pipeline(false);
        let normal = GrayLut::build(&pipeline, None, Some(16));

        pipeline.photometric = Photometric::Monochrome1;
        let inverted = GrayLut::build(&pipeline, None, Some(16));

        for raw in [0_u16, 424, 1024, 4095, 65535] {
            let sum = u16::from(normal.lookup(raw)) + u16::from(inverted.lookup(raw));
            // 允许差 1:归一化值恰为 0.5 时两边都 round 到 128(见 display 模块)
            assert!(
                (255..=256).contains(&sum),
                "raw={raw} 时两表应互补,实际 {} 和 {}",
                normal.lookup(raw),
                inverted.lookup(raw)
            );
        }
    }

    /// 越界查表返回 0,不 panic —— 一个坏像素不该让整帧渲染失败。
    #[test]
    fn out_of_range_lookup_is_graceful() {
        let pipeline = ct_pipeline(false);
        let lut = GrayLut::build(&pipeline, None, Some(8));
        assert_eq!(lut.len(), 256);
        // 8 位表查 16 位值
        assert_eq!(lut.lookup(300), 0);
        assert_eq!(lut.lookup(65535), 0);
    }

    /// 位宽被夹在合理范围内,不会因为设备送出荒谬的 BitsAllocated 而爆内存。
    #[test]
    fn absurd_bit_depths_are_clamped() {
        let pipeline = ct_pipeline(false);

        // 32 位会是 4G 项的表 —— 必须夹到 16
        let huge = GrayLut::build(&pipeline, None, Some(32));
        assert_eq!(huge.bits, MAX_LUT_BITS);
        assert_eq!(huge.len(), 65536);

        // 0 位夹到 1
        let zero = GrayLut::build(&pipeline, None, Some(0));
        assert_eq!(zero.bits, 1);
        assert_eq!(zero.len(), 2);
    }

    /// 表就是字节,能零拷贝交给自定义协议。
    #[test]
    fn table_is_directly_transferable_as_bytes() {
        let pipeline = ct_pipeline(false);
        let lut = GrayLut::build(&pipeline, None, Some(16));
        assert_eq!(lut.as_bytes().len(), 65536);
        assert_eq!(lut.as_bytes()[424], lut.lookup(424));
    }

    /// 缺 bits_allocated 时按 bits_stored 推:≤8 用 8 位表,否则 16 位。
    #[test]
    fn missing_bits_allocated_falls_back_to_bits_stored() {
        let mut pipeline = ct_pipeline(false);

        pipeline.bits_stored = 8;
        assert_eq!(GrayLut::build(&pipeline, None, None).bits, 8);

        pipeline.bits_stored = 12;
        assert_eq!(GrayLut::build(&pipeline, None, None).bits, 16);

        pipeline.bits_stored = 16;
        assert_eq!(GrayLut::build(&pipeline, None, None).bits, 16);
    }
}
