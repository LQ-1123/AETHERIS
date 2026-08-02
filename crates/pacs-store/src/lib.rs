//! 影像文件落盘。
//!
//! # 为什么顺序这么讲究
//!
//! C-STORE 回 `0x0000` 是对发送方的承诺:这份影像已经安全存下,你可以删本地
//! 副本了。设备真的会照做。所以在回成功之前,数据必须已经落到持久介质上,
//! 而不是还躺在页缓存里等着断电时消失。
//!
//! 落盘顺序:
//!
//! 1. 写临时文件 → `fsync(file)` —— 文件内容落盘
//! 2. `rename()` 到最终路径 → `fsync(parent_dir)` —— rename 本身是原子的,
//!    但目录项什么时候落盘不保证,少了这步崩溃后可能文件在、目录里查不到
//! 3. 数据库事务提交
//! 4. 才发 C-STORE-RSP 成功
//!
//! 本 crate 负责 1 和 2。任一步失败都不留半截数据:临时文件还在 `.tmp/` 下,
//! 最终路径上什么都没有,启动时的 [`Store::cleanup_temp`] 会清掉残留。

pub mod layout;

use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

pub use layout::InstanceKey;

/// 临时文件目录名,位于存储根下。
///
/// 和最终路径同处一个文件系统是硬性要求 —— `rename()` 只在同一文件系统内
/// 才是原子的,跨设备会退化成复制,那就失去了原子性。
pub const TEMP_DIR: &str = ".tmp";

/// 影像文件存储。
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

/// 一次落盘的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFile {
    /// 相对存储根的路径,存进数据库的就是它 —— 根目录可以整体迁移。
    pub relative_path: String,
    pub size: u64,
    pub sha256: [u8; 32],
    pub outcome: StoreOutcome,
}

/// 目标路径上原本有没有东西。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreOutcome {
    /// 新文件。
    Created,
    /// 已存在且内容完全一致 —— 设备重传,幂等跳过,没有写盘。
    AlreadyIdentical,
    /// 已存在但内容不同,已覆盖。
    ///
    /// 同一个 SOPInstanceUID 对应两份不同的数据,按标准这不该发生,通常是
    /// 设备重用了 UID 或中途改写过影像。覆盖是为了让存储和数据库保持一致,
    /// 但一定会告警 —— 这是设备侧的 bug 信号。
    Replaced,
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("存储路径 {path} 操作失败")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("相对路径 {relative:?} 越出了存储根")]
    PathEscape { relative: String },
    /// 数据库有记录、盘上没文件。
    ///
    /// 单独成一类是因为它的含义和其他 IO 错误不同:这是存储与数据库不一致的
    /// 信号(孤儿记录),调用方应当回 404 并告警,而不是当成一般的读失败。
    #[error("相对路径 {relative:?} 对应的文件不存在")]
    NotFound { relative: String },
}

impl StoreError {
    fn at(path: impl Into<PathBuf>) -> impl FnOnce(io::Error) -> Self {
        let path = path.into();
        move |source| Self::Io { path, source }
    }
}

impl Store {
    /// 打开(必要时创建)存储根。
    pub async fn open(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        fs::create_dir_all(&root)
            .await
            .map_err(StoreError::at(&root))?;

        let temp = root.join(TEMP_DIR);
        fs::create_dir_all(&temp)
            .await
            .map_err(StoreError::at(&temp))?;

        // 规范化根路径,后面 strip_prefix 才对得上(比如根是符号链接或含 `..`)
        let root = fs::canonicalize(&root)
            .await
            .map_err(StoreError::at(&root))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 把一个实例落盘,返回后内容已确实持久化。
    pub async fn store(
        &self,
        key: InstanceKey<'_>,
        bytes: &[u8],
    ) -> Result<StoredFile, StoreError> {
        let relative = layout::relative_path(key);
        let final_path = self.root.join(&relative);
        let sha256: [u8; 32] = Sha256::digest(bytes).into();
        let size = bytes.len() as u64;

        // 重传是常态,先看目标路径上有没有一模一样的东西
        let existing = self.compare_existing(&final_path, bytes, size).await?;
        match existing {
            Some(StoreOutcome::AlreadyIdentical) => {
                return Ok(StoredFile {
                    relative_path: relative,
                    size,
                    sha256,
                    outcome: StoreOutcome::AlreadyIdentical,
                });
            }
            Some(_) => tracing::warn!(
                path = %relative,
                "同一 SOPInstanceUID 收到内容不同的影像,覆盖旧文件;这通常是发送方重用了 UID"
            ),
            None => {}
        }

        // 1. 写临时文件并 fsync,内容先落盘
        let temp_path = self
            .root
            .join(TEMP_DIR)
            .join(format!("{}.part", Uuid::new_v4()));
        self.write_temp(&temp_path, bytes).await.inspect_err(|_| {
            // 写失败就别留垃圾;清不掉也没关系,启动时的 cleanup_temp 会兜底
            let temp_path = temp_path.clone();
            tokio::spawn(async move { fs::remove_file(temp_path).await });
        })?;

        // 2. 建目录 → rename → fsync 父目录,让目录项也落盘
        let parent = final_path.parent().expect("最终路径一定有父目录");
        self.create_dirs_synced(parent).await?;
        fs::rename(&temp_path, &final_path)
            .await
            .map_err(StoreError::at(&final_path))?;
        sync_dir(parent).await.map_err(StoreError::at(parent))?;

        Ok(StoredFile {
            relative_path: relative,
            size,
            sha256,
            outcome: if existing.is_some() {
                StoreOutcome::Replaced
            } else {
                StoreOutcome::Created
            },
        })
    }

    /// 目标路径已有文件时,判断内容是否一致。路径为空返回 `None`。
    async fn compare_existing(
        &self,
        final_path: &Path,
        bytes: &[u8],
        size: u64,
    ) -> Result<Option<StoreOutcome>, StoreError> {
        let metadata = match fs::metadata(final_path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(StoreError::at(final_path)(error)),
        };

        // 先比长度:不等就不用把整个文件读进内存了
        if metadata.len() != size {
            return Ok(Some(StoreOutcome::Replaced));
        }
        let existing = fs::read(final_path)
            .await
            .map_err(StoreError::at(final_path))?;
        Ok(Some(if existing == bytes {
            StoreOutcome::AlreadyIdentical
        } else {
            StoreOutcome::Replaced
        }))
    }

    async fn write_temp(&self, temp_path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
        let mut file = fs::File::create(temp_path)
            .await
            .map_err(StoreError::at(temp_path))?;
        file.write_all(bytes)
            .await
            .map_err(StoreError::at(temp_path))?;
        // 这一步才是"内容真的在盘上"。macOS 上 Rust 会用 F_FULLFSYNC,
        // 绕过磁盘自己的写缓存。
        file.sync_all().await.map_err(StoreError::at(temp_path))?;
        Ok(())
    }

    /// 逐级建目录,每新建一级就 fsync 它的父目录。
    ///
    /// `create_dir_all` 不会告诉你哪几级是新建的,而新建的目录项同样需要
    /// fsync 才持久 —— 少了这步,崩溃后可能整棵子目录都不见。同一个 series
    /// 的后续实例走的是已存在分支,不额外付出代价。
    async fn create_dirs_synced(&self, target: &Path) -> Result<(), StoreError> {
        let relative = target
            .strip_prefix(&self.root)
            .map_err(|_| StoreError::PathEscape {
                relative: target.display().to_string(),
            })?;

        let mut current = self.root.clone();
        for component in relative.components() {
            let parent = current.clone();
            current.push(component);
            match fs::create_dir(&current).await {
                Ok(()) => sync_dir(&parent).await.map_err(StoreError::at(&parent))?,
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => return Err(StoreError::at(&current)(error)),
            }
        }
        Ok(())
    }

    /// 把相对路径还原成绝对路径。**不检查文件是否存在,也不跟随符号链接。**
    ///
    /// 即使路径来自我们自己的数据库也要挡一道:库被写坏或迁移出错时,
    /// 一个含 `..` 的路径能让 WADO 读到存储根之外的任意文件。
    ///
    /// 读回文件请用 [`Store::resolve_for_read`] —— 它额外挡符号链接逃逸。
    pub fn resolve(&self, relative: &str) -> Result<PathBuf, StoreError> {
        let candidate = Path::new(relative);
        let escapes = candidate.is_absolute()
            || candidate
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)));
        if escapes {
            return Err(StoreError::PathEscape {
                relative: relative.to_owned(),
            });
        }
        Ok(self.root.join(candidate))
    }

    /// 解析出可以安全读取的绝对路径。
    ///
    /// 比 [`Store::resolve`] 多两道:
    ///
    /// 1. **canonicalize 后再验一次是否still在根内**。仅检查路径分量挡不住
    ///    符号链接 —— 存储根里若有一个 `evil.dcm -> /etc/passwd`,
    ///    它的每个分量都是 `Normal`,组件检查完全放行,跟随后却读到了根外。
    ///    canonicalize 会展开所有链接,展开后的真实路径才是判断依据。
    /// 2. 文件不存在时返回 [`StoreError::NotFound`],让调用方能区分
    ///    「数据库有记录但盘上没文件」(需要告警的不一致)和「路径非法」。
    ///
    /// 代价是一次 `realpath(2)` 系统调用。WADO 的每次取回都要读文件,
    /// 相比读取本身这点开销可以忽略。
    pub async fn resolve_for_read(&self, relative: &str) -> Result<PathBuf, StoreError> {
        let candidate = self.resolve(relative)?;

        let canonical = match fs::canonicalize(&candidate).await {
            Ok(path) => path,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Err(StoreError::NotFound {
                    relative: relative.to_owned(),
                });
            }
            Err(error) => return Err(StoreError::at(&candidate)(error)),
        };

        // 根本身在 open() 里已经 canonicalize 过,两边可比
        if !canonical.starts_with(&self.root) {
            tracing::error!(
                relative,
                resolved = %canonical.display(),
                root = %self.root.display(),
                "存储路径经符号链接越出了存储根,已拒绝"
            );
            return Err(StoreError::PathEscape {
                relative: relative.to_owned(),
            });
        }
        Ok(canonical)
    }

    /// 读回一个已落盘的实例。
    pub async fn read(&self, relative: &str) -> Result<Vec<u8>, StoreError> {
        let path = self.resolve_for_read(relative).await?;
        fs::read(&path).await.map_err(StoreError::at(&path))
    }

    /// 清掉 `.tmp/` 下的残留,返回删除数量。
    ///
    /// 崩溃恢复用:临时文件的存在说明当时那次落盘没走完,对应的数据库事务
    /// 也就没提交过,直接删是安全的。应该在服务启动时调用一次。
    pub async fn cleanup_temp(&self) -> Result<usize, StoreError> {
        let temp = self.root.join(TEMP_DIR);
        let mut entries = fs::read_dir(&temp).await.map_err(StoreError::at(&temp))?;
        let mut removed = 0;

        while let Some(entry) = entries.next_entry().await.map_err(StoreError::at(&temp))? {
            let path = entry.path();
            if path.is_file() {
                fs::remove_file(&path)
                    .await
                    .map_err(StoreError::at(&path))?;
                removed += 1;
            }
        }
        if removed > 0 {
            tracing::info!(removed, "清理了上次未完成落盘留下的临时文件");
        }
        Ok(removed)
    }
}

/// fsync 一个目录,让其中的目录项变更持久化。
#[cfg(unix)]
async fn sync_dir(path: &Path) -> io::Result<()> {
    fs::File::open(path).await?.sync_all().await
}

/// Windows 不允许对目录取句柄做 fsync,只能依赖文件系统自身的日志。
#[cfg(not(unix))]
async fn sync_dir(_path: &Path) -> io::Result<()> {
    Ok(())
}
