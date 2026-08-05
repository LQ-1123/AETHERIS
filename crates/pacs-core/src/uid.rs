//! DICOM 唯一标识符(UI VR)。
//!
//! 校验不是格式洁癖。UID 会直接作为文件路径分量使用(见 `pacs-store` 的存储
//! 布局),而 UID 来自外部影像设备 —— 不校验就拼路径,等于把"写到哪里"的
//! 控制权交给对端。本模块保证:任何构造成功的 `Uid` 都是安全的单级路径名
//! —— 非空、不含 `/` `\` NUL 等任何非数字非点字符、不是 `.` 或 `..`、
//! 长度不超过 64。

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// UI VR 的最大长度(PS3.5 §6.2 表 6.2-1)。
pub const MAX_LEN: usize = 64;

/// 一个校验通过的 DICOM UID。
///
/// 反序列化同样走校验,因此这个类型在任何来源下都保持上述不变式。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct Uid(String);

/// UID 校验失败的原因。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UidError {
    #[error("UID 为空")]
    Empty,
    #[error("UID 长度 {len} 超过上限 {MAX_LEN}")]
    TooLong { len: usize },
    #[error("UID 含非法字符 {ch:?}(只允许 ASCII 数字与 `.`)")]
    InvalidChar { ch: char },
    #[error("UID 含空分量(首尾是点,或出现连续点)")]
    EmptyComponent,
}

impl Uid {
    /// 校验并构造。
    ///
    /// 会先去掉 DICOM 的补齐字符:值长度必须为偶数,奇数时用 NUL 或空格补一位。
    ///
    /// 分量前导零(如 `1.02.3`)在 PS3.5 中不合规,但真实设备常见,这里放行 ——
    /// 它不影响路径安全,拒绝反而会丢数据。
    pub fn parse(raw: &str) -> Result<Self, UidError> {
        let s = raw.trim_matches(|c: char| c == '\0' || c == ' ');

        if s.is_empty() {
            return Err(UidError::Empty);
        }
        if s.len() > MAX_LEN {
            return Err(UidError::TooLong { len: s.len() });
        }
        if let Some(ch) = s.chars().find(|c| !c.is_ascii_digit() && *c != '.') {
            return Err(UidError::InvalidChar { ch });
        }
        // 光有字符集检查不够:`.` 和 `..` 全由点组成,能通过上一步。
        // 拒绝空分量同时挡掉首尾点和连续点,这是路径安全的关键一步。
        if s.split('.').any(str::is_empty) {
            return Err(UidError::EmptyComponent);
        }

        Ok(Self(s.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }

    /// Generate a globally unique DICOM UID using the UUID-derived `2.25` root.
    pub fn generate() -> Self {
        // PS3.5 B.2 defines `2.25.<UUID as an unsigned decimal integer>`.
        Self(format!("2.25.{}", uuid::Uuid::new_v4().as_u128()))
    }
}

impl fmt::Display for Uid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for Uid {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Uid {
    type Error = UidError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl TryFrom<&str> for Uid {
    type Error = UidError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl FromStr for Uid {
    type Err = UidError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Component, Path};

    use super::*;

    #[test]
    fn accepts_well_formed_uids() {
        // Verification SOP Class,以及一个典型的设备生成 UID
        for raw in [
            "1.2.840.10008.1.1",
            "1.3.6.1.4.1.14519.5.2.1.7695.2311",
            "0",
        ] {
            assert!(Uid::parse(raw).is_ok(), "应接受 {raw:?}");
        }
    }

    #[test]
    fn generated_uid_is_valid_and_unique() {
        let first = Uid::generate();
        let second = Uid::generate();
        assert_ne!(first, second);
        assert!(first.as_str().starts_with("2.25."));
        assert!(Uid::parse(first.as_str()).is_ok());
    }

    #[test]
    fn accepts_leading_zero_components() {
        // 不合规但真实存在,放行而不是丢数据
        assert_eq!(Uid::parse("1.02.3").unwrap().as_str(), "1.02.3");
    }

    #[test]
    fn strips_dicom_padding() {
        // 奇数长度的值会被补一个 NUL 或空格
        assert_eq!(Uid::parse("1.2.840\0").unwrap().as_str(), "1.2.840");
        assert_eq!(Uid::parse(" 1.2.840 ").unwrap().as_str(), "1.2.840");
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(Uid::parse(""), Err(UidError::Empty));
        assert_eq!(Uid::parse("\0\0"), Err(UidError::Empty));
    }

    #[test]
    fn rejects_overlong() {
        let raw = "1".repeat(MAX_LEN + 1);
        assert_eq!(Uid::parse(&raw), Err(UidError::TooLong { len: 65 }));
        assert!(Uid::parse(&"1".repeat(MAX_LEN)).is_ok());
    }

    #[test]
    fn rejects_non_uid_characters() {
        for raw in ["1.2.a", "1.2-3", "1.2 3", "1.2\n3"] {
            assert!(
                matches!(Uid::parse(raw), Err(UidError::InvalidChar { .. })),
                "应拒绝 {raw:?}"
            );
        }
    }

    /// 路径穿越是这个类型存在的首要理由,单独测。
    #[test]
    fn rejects_path_traversal_attempts() {
        for raw in [
            ".",
            "..",
            "../..",
            "1..2",
            ".1.2",
            "1.2.",
            "/etc/passwd",
            "..\\..\\x",
            "1.2/../../etc",
            "\0",
            "~",
        ] {
            assert!(Uid::parse(raw).is_err(), "应拒绝 {raw:?}");
        }
    }

    /// 不变式:任何构造成功的 Uid 都是单级、普通的路径分量。
    #[test]
    fn valid_uids_are_safe_path_components() {
        for raw in ["1.2.840.10008.1.1", "0", "1.02.3", &"9".repeat(MAX_LEN)] {
            let uid = Uid::parse(raw).expect("应接受");
            let path = Path::new(uid.as_str());
            let components: Vec<_> = path.components().collect();
            assert_eq!(components.len(), 1, "{raw:?} 应只有一个路径分量");
            assert!(
                matches!(components[0], Component::Normal(_)),
                "{raw:?} 应是普通分量,而不是 RootDir/ParentDir/CurDir"
            );
            assert!(path.is_relative(), "{raw:?} 应是相对路径");
        }
    }

    #[test]
    fn deserialization_validates() {
        assert!(serde_json::from_str::<Uid>("\"1.2.840\"").is_ok());
        assert!(serde_json::from_str::<Uid>("\"../etc\"").is_err());
    }
}
