//! 账号模型与角色权限。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 角色。
///
/// 刻意做成固定枚举而不是「权限位任意组合」:医疗系统的角色划分是稳定的,
/// 可自由组合的权限位会让「这个人到底能干什么」变得难以审计 ——
/// 而合规检查恰恰要回答这个问题。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// 系统管理员:管账号、看审计、删影像。
    Admin,
    /// 放射科医师:读片、写报告。
    Radiologist,
    /// 技师:上传影像。
    Technician,
    /// 只读:仅查看。
    Viewer,
}

/// 一项具体权限。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    /// 查询与查看影像。
    ViewImages,
    /// 上传影像(STOW-RS)。
    UploadImages,
    /// 撰写与修改报告。
    WriteReport,
    /// 创建、停用、改密其他账号。
    ManageUsers,
    /// 查阅审计日志。
    ViewAuditLog,
    /// 删除影像。
    DeleteImages,
    /// 修改临床白名单内的 DICOM 标签。
    EditDicomTags,
    /// 查看 DICOM 修订历史。
    ViewDicomRevisions,
}

impl Role {
    pub const ALL: &'static [Role] = &[
        Role::Admin,
        Role::Radiologist,
        Role::Technician,
        Role::Viewer,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Radiologist => "radiologist",
            Self::Technician => "technician",
            Self::Viewer => "viewer",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "admin" => Self::Admin,
            "radiologist" => Self::Radiologist,
            "technician" => Self::Technician,
            "viewer" => Self::Viewer,
            _ => return None,
        })
    }

    /// 这个角色是否具备某项权限。
    ///
    /// 权限矩阵集中在这一处,不散落到各个接口 —— 散落之后就没人能说清
    /// 某个角色究竟能做什么。
    pub fn can(self, permission: Permission) -> bool {
        use Permission::*;
        match self {
            Self::Admin => true,
            // 医师读片写报告,但不管账号、不删影像。删除是不可逆的,
            // 且删影像属于数据生命周期管理,不是临床工作的一部分。
            Self::Radiologist => {
                matches!(permission, ViewImages | WriteReport | ViewDicomRevisions)
            }
            // 技师负责把设备产出的影像送进来,不参与诊断
            Self::Technician => matches!(
                permission,
                ViewImages | UploadImages | EditDicomTags | ViewDicomRevisions
            ),
            Self::Viewer => matches!(permission, ViewImages),
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 一个账号。不含密码哈希 —— 那个只在校验时从库里单独取。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct User {
    pub id: i64,
    pub institution_id: i64,
    pub username: String,
    pub display_name: Option<String>,
    pub role: Role,
    pub is_active: bool,
    /// 管理员重置密码后为 true,用户下次登录必须先改密。
    pub must_change_password: bool,
    pub last_login_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// 用户名规则。
///
/// 限制字符集是为了避免用户名被用在别处时产生歧义(日志、审计、文件名);
/// 长度下限防手滑,上限防滥用。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InvalidUsername {
    #[error("用户名至少 3 个字符")]
    TooShort,
    #[error("用户名最多 64 个字符")]
    TooLong,
    #[error("用户名只允许小写字母、数字、`.` `_` `-`,且须以字母或数字开头")]
    IllegalCharacters,
}

pub const USERNAME_MIN_LEN: usize = 3;
pub const USERNAME_MAX_LEN: usize = 64;

/// 校验并规范化用户名(转小写)。
///
/// 统一转小写是为了避免 `Alice` 和 `alice` 被当成两个账号 —— 那既会让
/// 审计混乱,也是一种钓鱼手法。
pub fn normalize_username(raw: &str) -> Result<String, InvalidUsername> {
    let trimmed = raw.trim().to_lowercase();

    if trimmed.chars().count() < USERNAME_MIN_LEN {
        return Err(InvalidUsername::TooShort);
    }
    if trimmed.chars().count() > USERNAME_MAX_LEN {
        return Err(InvalidUsername::TooLong);
    }
    let legal = trimmed
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'));
    let starts_well = trimmed
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    if !legal || !starts_well {
        return Err(InvalidUsername::IllegalCharacters);
    }
    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_names_round_trip() {
        for role in Role::ALL {
            assert_eq!(Role::parse(role.as_str()), Some(*role));
        }
        assert_eq!(Role::parse("superuser"), None);
    }

    /// 角色名会写进数据库的 CHECK 约束,两边必须一致。
    #[test]
    fn role_names_match_the_database_constraint() {
        let migration = include_str!("../../pacs-db/migrations/0002_accounts.sql");
        for role in Role::ALL {
            assert!(
                migration.contains(&format!("'{}'", role.as_str())),
                "迁移里的 CHECK 约束缺少角色 {role}"
            );
        }
    }

    #[test]
    fn admin_can_do_everything() {
        for permission in [
            Permission::ViewImages,
            Permission::UploadImages,
            Permission::WriteReport,
            Permission::ManageUsers,
            Permission::ViewAuditLog,
            Permission::DeleteImages,
            Permission::EditDicomTags,
            Permission::ViewDicomRevisions,
        ] {
            assert!(Role::Admin.can(permission));
        }
    }

    /// 账号管理和删影像只有管理员能做 —— 这两项写错的后果最重。
    #[test]
    fn privileged_actions_are_admin_only() {
        for role in [Role::Radiologist, Role::Technician, Role::Viewer] {
            assert!(!role.can(Permission::ManageUsers), "{role} 不该能管账号");
            assert!(!role.can(Permission::DeleteImages), "{role} 不该能删影像");
            assert!(
                !role.can(Permission::ViewAuditLog),
                "{role} 不该能看审计日志"
            );
        }
    }

    #[test]
    fn every_role_can_view_images() {
        for role in Role::ALL {
            assert!(role.can(Permission::ViewImages), "{role} 应能查看影像");
        }
    }

    #[test]
    fn upload_is_limited_to_technicians_and_admins() {
        assert!(Role::Technician.can(Permission::UploadImages));
        assert!(Role::Admin.can(Permission::UploadImages));
        assert!(!Role::Radiologist.can(Permission::UploadImages));
        assert!(!Role::Viewer.can(Permission::UploadImages));
    }

    #[test]
    fn dicom_revision_permissions_match_the_clinical_roles() {
        assert!(Role::Admin.can(Permission::EditDicomTags));
        assert!(Role::Technician.can(Permission::EditDicomTags));
        assert!(!Role::Radiologist.can(Permission::EditDicomTags));
        assert!(!Role::Viewer.can(Permission::EditDicomTags));

        assert!(Role::Admin.can(Permission::ViewDicomRevisions));
        assert!(Role::Technician.can(Permission::ViewDicomRevisions));
        assert!(Role::Radiologist.can(Permission::ViewDicomRevisions));
        assert!(!Role::Viewer.can(Permission::ViewDicomRevisions));
    }

    #[test]
    fn usernames_are_lowercased() {
        // Alice 和 alice 必须是同一个账号,否则审计对不上,也给了钓鱼空间
        assert_eq!(normalize_username("Alice").unwrap(), "alice");
        assert_eq!(normalize_username("  Bob.Smith  ").unwrap(), "bob.smith");
    }

    #[test]
    fn rejects_bad_usernames() {
        assert_eq!(normalize_username("ab"), Err(InvalidUsername::TooShort));
        assert_eq!(
            normalize_username(&"a".repeat(65)),
            Err(InvalidUsername::TooLong)
        );
        for bad in [
            "-leading",
            "_leading",
            ".leading",
            "has space",
            "有中文",
            "a@b",
        ] {
            assert_eq!(
                normalize_username(bad),
                Err(InvalidUsername::IllegalCharacters),
                "应拒绝 {bad:?}"
            );
        }
    }

    #[test]
    fn accepts_reasonable_usernames() {
        for good in ["abc", "zhang.san", "tech_01", "user-1", "a1b2c3"] {
            assert!(normalize_username(good).is_ok(), "应接受 {good:?}");
        }
    }
}
