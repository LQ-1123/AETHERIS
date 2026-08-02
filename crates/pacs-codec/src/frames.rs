//! 帧提取。
//!
//! # 帧号从 1 开始
//!
//! DICOM 和 WADO-RS 的帧号都是 **1 基**(PS3.18 §10.4.1):`/frames/1` 是第一帧,
//! `/frames/0` 是非法请求。而 `dicom-pixeldata` 的 `frame_data()` 是 0 基。
//!
//! 这个差一位的错误特别值得单独防:单帧影像上表现完全正常(第 1 帧当成第 0 帧
//! 恰好也是对的),只有多帧的 CT/MR 序列才会显现 —— 每一帧都偏移一位,
//! 最后一帧永远取不到。等到那时候已经很难联想到是帧号基准的问题了。
//!
//! # 为什么返回解码后的字节
//!
//! `/frames` 配 `Accept: application/octet-stream` 要的是未压缩的原始帧
//! (PS3.18 §10.4.1.1.1)。压缩传输语法要按原样转发压缩帧的话,得解析
//! encapsulated pixel data 的分片和 basic offset table,那是另一件事;
//! 而查看器拿到未压缩帧就能直接渲染,不必内置各种解码器。

use dicom::object::{FileDicomObject, InMemDicomObject};
use dicom_pixeldata::PixelDecoder;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FrameError {
    /// 帧号是 1 基,0 一定是调用方搞错了基准。
    #[error("帧号从 1 开始,收到 0")]
    ZeroFrameNumber,
    #[error("帧号 {requested} 超出范围,该实例共 {total} 帧")]
    OutOfRange { requested: u32, total: u32 },
    #[error("像素数据解码失败")]
    Decode {
        #[source]
        // 装箱:dicom-pixeldata 的错误类型很大,不装箱会让每个 Result 都变胖
        source: Box<dicom_pixeldata::Error>,
    },
}

/// 一个实例里可供提取的帧。
///
/// 解码一次、多帧共用:一次 WADO 请求常常要 `/frames/1,2,3`,
/// 每帧都重新解码整个实例的话,代价是帧数的倍数。
pub struct Frames<'a> {
    // 借用源对象:未压缩语法下 `frame_data()` 直接切原始缓冲,不额外复制。
    // 因此 `Frames` 不能比它解码的那个对象活得更久。
    decoded: dicom_pixeldata::DecodedPixelData<'a>,
}

impl<'a> Frames<'a> {
    /// 解码一个实例的像素数据。
    ///
    /// CPU 密集(压缩语法要走解码器),调用方必须放到 `spawn_blocking` 里 ——
    /// 直接跑在 async executor 上会把整个 runtime 卡住几十毫秒。
    pub fn decode(object: &'a FileDicomObject<InMemDicomObject>) -> Result<Self, FrameError> {
        let decoded = object
            .decode_pixel_data()
            .map_err(|source| FrameError::Decode {
                source: Box::new(source),
            })?;
        Ok(Self { decoded })
    }

    pub fn total(&self) -> u32 {
        self.decoded.number_of_frames()
    }

    /// 取第 `number` 帧(**1 基**)的未压缩字节。
    pub fn frame(&self, number: u32) -> Result<&[u8], FrameError> {
        if number == 0 {
            return Err(FrameError::ZeroFrameNumber);
        }
        let total = self.total();
        if number > total {
            return Err(FrameError::OutOfRange {
                requested: number,
                total,
            });
        }
        // 这里是唯一的基准转换点。放在一处,别的地方就不必再关心基准。
        self.decoded
            .frame_data(number - 1)
            .map_err(|source| FrameError::Decode {
                source: Box::new(source),
            })
    }
}

#[cfg(test)]
mod tests {
    // 唯一的测试需要 fixtures feature,不开时整块都不编译 ——
    // 所以 use 也必须一起受控,否则会是未使用的导入。
    #[cfg(feature = "fixtures")]
    #[test]
    fn frame_numbers_are_one_based() {
        use super::*;
        use pacs_core::fixture::{ct_instance, unique_uid};

        let object = ct_instance(&unique_uid(), &unique_uid(), &unique_uid());
        let frames = Frames::decode(&object).expect("夹具应能解码");
        assert_eq!(frames.total(), 1, "夹具是单帧");

        // 第 1 帧存在
        let first = frames.frame(1).expect("第 1 帧应当存在");
        // 4×4、16 位、单采样 = 32 字节
        assert_eq!(first.len(), 4 * 4 * 2);

        // 0 是非法帧号,不能当成第一帧
        assert!(
            matches!(frames.frame(0), Err(FrameError::ZeroFrameNumber)),
            "帧号 0 必须报错 —— 当成第 1 帧会让所有多帧影像偏移一位"
        );
        // 超出范围
        assert!(matches!(
            frames.frame(2),
            Err(FrameError::OutOfRange {
                requested: 2,
                total: 1
            })
        ));
    }
}
