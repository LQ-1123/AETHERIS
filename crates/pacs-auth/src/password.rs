//! 密码哈希与强度策略。

use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use std::sync::LazyLock;
use thiserror::Error;

/// 密码长度下限。
///
/// NIST SP 800-63B 的立场是**长度优先于复杂度**:强制大小写+数字+符号会把用户
/// 推向 `Password1!` 这类可预测的模式,反而更弱。所以这里只要长度,
/// 不做字符类别要求。
pub const MIN_LEN: usize = 12;

/// 密码长度上限。
///
/// 不是安全考虑而是拒绝服务考虑:argon2 的耗时随输入增长,
/// 没有上限的话一个 10 MB 的"密码"就能占住一个 CPU 核。
pub const MAX_LEN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WeakPassword {
    #[error("密码至少 {MIN_LEN} 个字符")]
    TooShort,
    #[error("密码最多 {MAX_LEN} 个字符")]
    TooLong,
    #[error("密码不能只由同一个字符重复组成")]
    Repetitive,
    #[error("密码不能包含用户名")]
    ContainsUsername,
}

#[derive(Debug, Error)]
pub enum PasswordError {
    #[error("密码哈希失败")]
    Hash(#[source] argon2::password_hash::Error),
    #[error("存储的密码哈希格式无效")]
    MalformedHash(#[source] argon2::password_hash::Error),
}

/// argon2id 参数。
///
/// 用默认值:argon2 0.5 的默认是 argon2id v19、内存 19 MiB、迭代 2 次、并行 1,
/// 正好是 OWASP 的推荐下限。参数会随硬件演进,而它们被写进哈希串本身,
/// 所以将来调高不会导致老密码验不了。
static ARGON2: LazyLock<Argon2<'static>> = LazyLock::new(Argon2::default);

/// 生成 PHC 格式的密码哈希(自带随机盐与参数)。
pub fn hash(password: &str) -> Result<String, PasswordError> {
    let salt = SaltString::generate(&mut OsRng);
    ARGON2
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(PasswordError::Hash)
}

/// 校验密码。
pub fn verify(password: &str, stored_hash: &str) -> Result<bool, PasswordError> {
    let parsed = PasswordHash::new(stored_hash).map_err(PasswordError::MalformedHash)?;
    Ok(ARGON2.verify_password(password.as_bytes(), &parsed).is_ok())
}

/// 用户名不存在时也要付出一次同等的哈希代价。
///
/// 否则「用户不存在」立刻返回、「密码错误」要等 argon2 跑完,
/// 攻击者用响应时间就能枚举出哪些账号存在。医疗系统的用户名往往就是
/// 员工工号或姓名拼音,泄露出去是有意义的信息。
pub fn waste_time_like_a_real_verification() {
    // 用一个固定的合法哈希去验一个固定的错密码,耗时与真实校验同量级
    static DECOY: LazyLock<String> =
        LazyLock::new(|| hash("decoy-password-never-matches").expect("固定输入应能哈希"));
    let _ = verify("wrong", &DECOY);
}

/// 强度检查。`username` 用于拒绝「密码里含用户名」。
pub fn check_strength(password: &str, username: &str) -> Result<(), WeakPassword> {
    let length = password.chars().count();
    if length < MIN_LEN {
        return Err(WeakPassword::TooShort);
    }
    if length > MAX_LEN {
        return Err(WeakPassword::TooLong);
    }

    let mut chars = password.chars();
    if let Some(first) = chars.next()
        && chars.all(|c| c == first)
    {
        return Err(WeakPassword::Repetitive);
    }

    if !username.is_empty() && password.to_lowercase().contains(&username.to_lowercase()) {
        return Err(WeakPassword::ContainsUsername);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_then_verify_succeeds() {
        let stored = hash("correct horse battery staple").unwrap();
        assert!(verify("correct horse battery staple", &stored).unwrap());
    }

    #[test]
    fn wrong_password_fails() {
        let stored = hash("correct horse battery staple").unwrap();
        assert!(!verify("Correct horse battery staple", &stored).unwrap());
        assert!(!verify("", &stored).unwrap());
    }

    /// 同一个密码两次哈希必须不同 —— 相同就说明盐没起作用,
    /// 一次泄露就能看出哪些人用了同样的密码。
    #[test]
    fn hashes_are_salted() {
        let a = hash("same-password-here").unwrap();
        let b = hash("same-password-here").unwrap();
        assert_ne!(a, b, "两次哈希相同说明没加盐");
        assert!(verify("same-password-here", &a).unwrap());
        assert!(verify("same-password-here", &b).unwrap());
    }

    /// 参数必须写在哈希串里,将来调高强度才不会让老密码失效。
    #[test]
    fn hash_is_phc_format_with_parameters() {
        let stored = hash("some-password-x").unwrap();
        assert!(
            stored.starts_with("$argon2id$"),
            "应使用 argon2id,实际:{stored}"
        );
        assert!(stored.contains("$m="), "哈希串里应带内存参数");
        assert!(stored.contains("t="), "哈希串里应带迭代次数");
    }

    #[test]
    fn malformed_stored_hash_is_an_error_not_a_pass() {
        // 库里存了垃圾时必须报错,绝不能"验不了就放行"
        assert!(matches!(
            verify("anything", "not-a-hash"),
            Err(PasswordError::MalformedHash(_))
        ));
        assert!(matches!(
            verify("anything", ""),
            Err(PasswordError::MalformedHash(_))
        ));
    }

    #[test]
    fn strength_requires_length() {
        assert_eq!(
            check_strength("short", "alice"),
            Err(WeakPassword::TooShort)
        );
        assert_eq!(
            check_strength(&"a".repeat(MAX_LEN + 1), "alice"),
            Err(WeakPassword::TooLong)
        );
        assert!(check_strength("a-perfectly-fine-passphrase", "alice").is_ok());
    }

    /// 只要够长就行,不强制字符类别 —— 见 MIN_LEN 的说明。
    #[test]
    fn long_all_lowercase_passphrase_is_accepted() {
        assert!(check_strength("correct horse battery staple", "alice").is_ok());
    }

    #[test]
    fn rejects_repetitive_and_username_bearing_passwords() {
        assert_eq!(
            check_strength(&"x".repeat(20), "alice"),
            Err(WeakPassword::Repetitive)
        );
        assert_eq!(
            check_strength("alice-my-password", "alice"),
            Err(WeakPassword::ContainsUsername)
        );
        // 大小写不同也算包含
        assert_eq!(
            check_strength("ALICE-my-password", "alice"),
            Err(WeakPassword::ContainsUsername)
        );
    }

    /// 用户不存在时的假校验必须真的耗时,否则响应时间会泄露账号是否存在。
    #[test]
    fn decoy_verification_costs_comparable_time() {
        let stored = hash("real-password-here").unwrap();

        let started = std::time::Instant::now();
        let _ = verify("wrong-password-xx", &stored);
        let real = started.elapsed();

        let started = std::time::Instant::now();
        waste_time_like_a_real_verification();
        let decoy = started.elapsed();

        // 同一个量级即可(CI 机器抖动大,不做严格比较);
        // 关键是假校验没有被优化成空操作
        assert!(
            decoy.as_micros() > real.as_micros() / 10,
            "假校验耗时 {decoy:?} 远小于真实校验 {real:?},时间侧信道仍然存在"
        );
    }
}
