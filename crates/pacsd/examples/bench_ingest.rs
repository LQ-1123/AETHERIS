//! 入库吞吐基准。
//!
//! 测的是 C-STORE 成功响应之前必须完成的那条链路:
//! **解析 → 落盘并 fsync → 数据库事务提交**。DIMSE 协议开销不在内 ——
//! 那部分受网络和对端实现影响,不是我们能优化的部分。
//!
//! ```sh
//! cargo run --release -p pacsd --example bench_ingest -- [实例数] [并发数] [边长]
//! ```
//!
//! 必须用 `--release`:debug 构建下 DICOM 解析慢一个数量级,量到的是编译选项
//! 而不是代码。
//!
//! 计划里的目标是未压缩 CT ≥200 instance/s。默认参数是 512×512×16bit
//! (≈512 KiB/实例),和真实 CT 断层同量级。

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use pacs_core::fixture::{ct_instance_sized, unique_uid};
use pacs_db::{StorageRecord, ingest_instance};
use pacs_store::{InstanceKey, Store};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let mut args = std::env::args().skip(1);
    let total: usize = parse_arg(args.next(), 200)?;
    let concurrency: usize = parse_arg(args.next(), 8)?;
    let side: u16 = parse_arg(args.next(), 512)?;

    let database_url = std::env::var("PACS_TEST_DATABASE_URL")
        .context("需要 PACS_TEST_DATABASE_URL —— 基准会写入大量数据,不要指向开发库")?;
    let pool = pacs_db::connect(&database_url).await?;
    pacs_db::migrate(&pool).await?;

    let storage_dir = tempfile::tempdir()?;
    let store = Store::open(storage_dir.path()).await?;

    // 先把实例全部造好再计时:DICOM 编码是准备工作,不属于入库开销
    println!("正在准备 {total} 份 {side}×{side} 实例…");
    let study_uid = unique_uid();
    let series_uid = unique_uid();
    let prepared: Vec<(String, Vec<u8>)> = (0..total)
        .map(|_| {
            let sop_uid = unique_uid();
            let object = ct_instance_sized(&study_uid, &series_uid, &sop_uid, side);
            let mut bytes = Vec::new();
            object.write_all(&mut bytes).expect("夹具应能编码");
            (sop_uid, bytes)
        })
        .collect();
    let bytes_each = prepared[0].1.len();
    println!(
        "单份 {:.1} KiB,共 {:.1} MiB;并发 {concurrency}\n",
        bytes_each as f64 / 1024.0,
        (bytes_each * total) as f64 / (1024.0 * 1024.0),
    );

    let store = Arc::new(store);
    let pool = Arc::new(pool);
    // 用信号量控制在途数量,而不是一次性 spawn 全部 —— 后者量到的是调度器
    // 抖动,不是稳态吞吐
    let permits = Arc::new(tokio::sync::Semaphore::new(concurrency));

    let started = Instant::now();
    let mut tasks = Vec::with_capacity(total);
    for (_sop_uid, bytes) in prepared {
        let permit = Arc::clone(&permits).acquire_owned().await?;
        let store = Arc::clone(&store);
        let pool = Arc::clone(&pool);
        tasks.push(tokio::spawn(async move {
            let _permit = permit;
            let began = Instant::now();
            let result = ingest_one(&store, &pool, &bytes).await;
            (began.elapsed(), result)
        }));
    }

    let mut latencies = Vec::with_capacity(total);
    let mut failures = 0_usize;
    for task in tasks {
        let (latency, result) = task.await?;
        match result {
            Ok(()) => latencies.push(latency),
            Err(error) => {
                failures += 1;
                if failures <= 3 {
                    eprintln!("入库失败:{error:#}");
                }
            }
        }
    }
    let wall = started.elapsed();

    latencies.sort_unstable();
    let succeeded = latencies.len();
    println!("耗时          {:.2}s", wall.as_secs_f64());
    println!("成功/失败     {succeeded} / {failures}");
    println!(
        "吞吐          {:.1} instance/s   {:.1} MiB/s",
        succeeded as f64 / wall.as_secs_f64(),
        (succeeded * bytes_each) as f64 / (1024.0 * 1024.0) / wall.as_secs_f64(),
    );
    if !latencies.is_empty() {
        println!(
            "单份延迟      p50 {:.1}ms   p95 {:.1}ms   p99 {:.1}ms   max {:.1}ms",
            millis(percentile(&latencies, 0.50)),
            millis(percentile(&latencies, 0.95)),
            millis(percentile(&latencies, 0.99)),
            millis(*latencies.last().unwrap()),
        );
    }

    anyhow::ensure!(failures == 0, "有 {failures} 份入库失败,吞吐数字不可信");
    Ok(())
}

/// 一份实例的完整入库链路,和 C-STORE 里的顺序一致。
async fn ingest_one(store: &Store, pool: &sqlx::PgPool, bytes: &[u8]) -> Result<()> {
    let object = dicom::object::from_reader(std::io::Cursor::new(bytes))?;
    let metadata = pacs_core::extract_metadata(&object)?;

    let stored = store
        .store(
            InstanceKey {
                study: &metadata.study.uid,
                series: &metadata.series.uid,
                sop: &metadata.instance.uid,
            },
            bytes,
        )
        .await?;

    ingest_instance(
        pool,
        &metadata,
        StorageRecord {
            relative_path: &stored.relative_path,
            size: stored.size,
            sha256: &stored.sha256,
        },
    )
    .await?;
    Ok(())
}

fn parse_arg<T: std::str::FromStr>(value: Option<String>, fallback: T) -> Result<T>
where
    T::Err: std::fmt::Display,
{
    match value {
        Some(raw) => raw
            .parse()
            .map_err(|error| anyhow::anyhow!("参数 {raw:?} 解析失败:{error}")),
        None => Ok(fallback),
    }
}

fn percentile(sorted: &[Duration], fraction: f64) -> Duration {
    let index = ((sorted.len() as f64 * fraction) as usize).min(sorted.len() - 1);
    sorted[index]
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
