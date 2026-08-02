//! 令牌:短命的 access token 与可吊销的 refresh token。
//!
//! # 为什么是两种令牌
//!
//! 纯 JWT 签发后无法吊销,只能等它过期。医疗场景里「员工离职」「设备丢失」
//! 要求**立刻**断访问,这是硬需求。所以:
//!
//! - **access token** —— JWT,15 分钟,无状态。每次请求都验签,不查库。
//!   短命把"吊销后仍可用"的窗口压到 15 分钟内。
//! - **refresh token** —— 256 位随机数,存库(存哈希)。可吊销、可轮换、
//!   可以列出某个用户的所有活跃会话。

use argon2::password_hash::rand_core::{OsRng, RngCore};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::model::Role;

/// access token 有效期。
///
/// 15 分钟是吊销延迟与刷新频率之间的折中:再短会让客户端频繁续期,
/// 再长则吊销后的可用窗口过大。
pub const ACCESS_TOKEN_TTL: Duration = Duration::minutes(15);

/// refresh token 有效期。超过这个时间未活动就要重新登录。
pub const REFRESH_TOKEN_TTL: Duration = Duration::days(14);

/// 签名密钥的最小长度。
///
/// HS256 的强度取决于密钥熵。太短的密钥可以离线暴力破解,
/// 破了就能自己签发任意身份的令牌 —— 包括 admin。
pub const MIN_SECRET_LEN: usize = 32;

#[derive(Debug, Error)]
pub enum TokenError {
    #[error(
        "签名密钥太短:{len} 字节,至少需要 {MIN_SECRET_LEN} 字节。用 `openssl rand -base64 48` 生成"
    )]
    WeakSecret { len: usize },
    #[error("令牌签发失败")]
    Encode(#[source] jsonwebtoken::errors::Error),
    #[error("令牌无效或已过期")]
    Invalid(#[source] jsonwebtoken::errors::Error),
    #[error("令牌里的角色 {role:?} 无法识别")]
    UnknownRole { role: String },
}

/// access token 的载荷。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessClaims {
    /// 用户 id。
    pub sub: i64,
    pub institution_id: i64,
    pub username: String,
    pub role: String,
    /// 签发时间(Unix 秒)。
    pub iat: i64,
    /// 过期时间(Unix 秒)。
    pub exp: i64,
}

impl AccessClaims {
    pub fn role(&self) -> Result<Role, TokenError> {
        Role::parse(&self.role).ok_or_else(|| TokenError::UnknownRole {
            role: self.role.clone(),
        })
    }
}

/// access token 的签发与校验。
#[derive(Clone)]
pub struct AccessTokenCodec {
    encoding: EncodingKey,
    decoding: DecodingKey,
    validation: Validation,
}

impl std::fmt::Debug for AccessTokenCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 不要把密钥打进日志
        f.write_str("AccessTokenCodec { .. }")
    }
}

impl AccessTokenCodec {
    /// 用共享密钥构造。密钥太短会直接报错而不是凑合着用。
    pub fn new(secret: &[u8]) -> Result<Self, TokenError> {
        if secret.len() < MIN_SECRET_LEN {
            return Err(TokenError::WeakSecret { len: secret.len() });
        }
        let mut validation = Validation::new(Algorithm::HS256);
        // 只接受 HS256。不限定算法的话,攻击者可以把头部改成 `none`
        // 或换成非对称算法用公钥当密钥 —— JWT 的经典漏洞。
        validation.algorithms = vec![Algorithm::HS256];
        validation.validate_exp = true;

        Ok(Self {
            encoding: EncodingKey::from_secret(secret),
            decoding: DecodingKey::from_secret(secret),
            validation,
        })
    }

    pub fn issue(
        &self,
        user_id: i64,
        institution_id: i64,
        username: &str,
        role: Role,
        now: DateTime<Utc>,
    ) -> Result<String, TokenError> {
        let claims = AccessClaims {
            sub: user_id,
            institution_id,
            username: username.to_owned(),
            role: role.as_str().to_owned(),
            iat: now.timestamp(),
            exp: (now + ACCESS_TOKEN_TTL).timestamp(),
        };
        jsonwebtoken::encode(&Header::new(Algorithm::HS256), &claims, &self.encoding)
            .map_err(TokenError::Encode)
    }

    pub fn verify(&self, token: &str) -> Result<AccessClaims, TokenError> {
        jsonwebtoken::decode::<AccessClaims>(token, &self.decoding, &self.validation)
            .map(|data| data.claims)
            .map_err(TokenError::Invalid)
    }
}

/// 一个刚生成的 refresh token。
///
/// 明文只在签发那一刻存在,交给客户端之后服务端只留哈希 ——
/// 数据库泄露时,拿到记录也无法冒充。
#[derive(Debug, Clone)]
pub struct RefreshToken {
    /// 给客户端的明文。
    pub secret: String,
    /// 入库的哈希。
    pub hash: Vec<u8>,
    pub expires_at: DateTime<Utc>,
}

/// 生成一个新的 refresh token。
pub fn generate_refresh_token(now: DateTime<Utc>) -> RefreshToken {
    // 256 位随机数。熵足够,不存在被猜中的可能。
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let secret = URL_SAFE_NO_PAD.encode(bytes);
    let hash = hash_refresh_token(&secret);
    RefreshToken {
        secret,
        hash,
        expires_at: now + REFRESH_TOKEN_TTL,
    }
}

/// refresh token 的入库哈希。
///
/// 用 SHA-256 而不是 argon2:令牌是 256 位随机数,没有可猜测的结构,
/// 慢哈希带不来额外安全性,却会让每次续期都付出几十毫秒 —— 而续期是热路径。
/// 密码不同,密码是人选的、熵低,才需要慢哈希。
pub fn hash_refresh_token(secret: &str) -> Vec<u8> {
    Sha256::digest(secret.as_bytes()).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codec() -> AccessTokenCodec {
        AccessTokenCodec::new(b"a-test-secret-that-is-long-enough-for-hs256").unwrap()
    }

    #[test]
    fn access_token_round_trips() {
        let now = Utc::now();
        let token = codec()
            .issue(42, 1, "alice", Role::Radiologist, now)
            .unwrap();
        let claims = codec().verify(&token).unwrap();

        assert_eq!(claims.sub, 42);
        assert_eq!(claims.institution_id, 1);
        assert_eq!(claims.username, "alice");
        assert_eq!(claims.role().unwrap(), Role::Radiologist);
    }

    #[test]
    fn short_secret_is_rejected() {
        assert!(matches!(
            AccessTokenCodec::new(b"too-short"),
            Err(TokenError::WeakSecret { .. })
        ));
        // 恰好达标应通过
        assert!(AccessTokenCodec::new(&[0_u8; MIN_SECRET_LEN]).is_ok());
    }

    #[test]
    fn expired_token_is_rejected() {
        let long_ago = Utc::now() - Duration::hours(2);
        let token = codec()
            .issue(1, 1, "alice", Role::Viewer, long_ago)
            .unwrap();
        assert!(matches!(
            codec().verify(&token),
            Err(TokenError::Invalid(_))
        ));
    }

    /// 换个密钥就应该验不过 —— 否则等于没验签。
    #[test]
    fn token_signed_with_another_key_is_rejected() {
        let token = codec()
            .issue(1, 1, "alice", Role::Admin, Utc::now())
            .unwrap();
        let other = AccessTokenCodec::new(b"a-different-secret-also-long-enough!!").unwrap();
        assert!(matches!(other.verify(&token), Err(TokenError::Invalid(_))));
    }

    /// `alg: none` 是 JWT 的经典漏洞:不锁定算法的话,
    /// 攻击者去掉签名就能伪造任意身份。
    #[test]
    fn unsigned_token_is_rejected() {
        // {"alg":"none","typ":"JWT"} + 一个 admin 载荷 + 空签名
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(
            format!(
                r#"{{"sub":1,"institution_id":1,"username":"attacker","role":"admin","iat":{},"exp":{}}}"#,
                Utc::now().timestamp(),
                (Utc::now() + Duration::hours(1)).timestamp()
            )
            .as_bytes(),
        );
        let forged = format!("{header}.{payload}.");
        assert!(
            matches!(codec().verify(&forged), Err(TokenError::Invalid(_))),
            "未签名的令牌必须被拒绝"
        );
    }

    /// 篡改载荷后签名就对不上了。
    #[test]
    fn tampered_payload_is_rejected() {
        let token = codec()
            .issue(1, 1, "alice", Role::Viewer, Utc::now())
            .unwrap();
        let mut parts: Vec<&str> = token.split('.').collect();
        let elevated = URL_SAFE_NO_PAD.encode(
            format!(
                r#"{{"sub":1,"institution_id":1,"username":"alice","role":"admin","iat":{},"exp":{}}}"#,
                Utc::now().timestamp(),
                (Utc::now() + Duration::hours(1)).timestamp()
            )
            .as_bytes(),
        );
        parts[1] = &elevated;
        assert!(matches!(
            codec().verify(&parts.join(".")),
            Err(TokenError::Invalid(_))
        ));
    }

    #[test]
    fn refresh_tokens_are_unique_and_hashed() {
        let now = Utc::now();
        let a = generate_refresh_token(now);
        let b = generate_refresh_token(now);

        assert_ne!(a.secret, b.secret, "两次生成不该相同");
        assert_ne!(a.hash, b.hash);
        assert_eq!(a.hash, hash_refresh_token(&a.secret));
        // 哈希不该能反推出明文
        assert!(!a.hash.starts_with(a.secret.as_bytes()));
        assert_eq!(a.hash.len(), 32);
    }

    #[test]
    fn refresh_token_has_enough_entropy() {
        let token = generate_refresh_token(Utc::now());
        // 256 位随机数用 base64url 无填充编码后是 43 个字符
        assert_eq!(token.secret.len(), 43);
    }

    #[test]
    fn refresh_token_expiry_follows_the_ttl() {
        let now = Utc::now();
        let token = generate_refresh_token(now);
        assert_eq!(token.expires_at, now + REFRESH_TOKEN_TTL);
    }

    /// 密钥不能出现在 Debug 输出里,否则一条日志就泄露了签名密钥。
    #[test]
    fn codec_debug_does_not_leak_the_secret() {
        let rendered = format!("{:?}", codec());
        assert!(!rendered.contains("secret"), "实际:{rendered}");
        assert!(!rendered.contains("long-enough"), "实际:{rendered}");
    }
}
