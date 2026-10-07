//! Local model manager for Ollama: hardware detection, a curated catalog with fit/speed
//! estimates for THIS machine, registry lookups, and pull/delete with progress.

use std::collections::HashSet;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfo {
    pub os: String,
    pub arch: String,
    pub cpu_cores: u32,
    pub total_ram_gb: f64,
    pub available_ram_gb: f64,
    /// "apple-silicon" | "nvidia" | "cpu"
    pub accelerator: String,
    /// Memory a model can realistically use on this machine.
    pub usable_for_models_gb: f64,
}

fn has_nvidia() -> bool {
    // No shelling out: the driver exposes this file on Linux. (Windows/other: treated as CPU for now.)
    std::path::Path::new("/proc/driver/nvidia/version").exists()
}

pub fn system_info() -> SystemInfo {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    let gb = |b: u64| (b as f64 / 1_073_741_824.0 * 10.0).round() / 10.0;
    let total = gb(sys.total_memory());
    let available = gb(sys.available_memory());
    let apple = cfg!(target_os = "macos") && cfg!(target_arch = "aarch64");
    let accelerator = if apple {
        "apple-silicon"
    } else if has_nvidia() {
        "nvidia"
    } else {
        "cpu"
    };
    // Apple Silicon lets the GPU use ~70% of unified memory; elsewhere leave ~4 GB for the OS and apps.
    let usable = if apple { total * 0.7 } else { (total - 4.0).max(total * 0.5) };
    SystemInfo {
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        cpu_cores: std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(4),
        total_ram_gb: total,
        available_ram_gb: available,
        accelerator: accelerator.into(),
        usable_for_models_gb: (usable * 10.0).round() / 10.0,
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogEntry {
    name: String,
    label: String,
    size_gb: f64,
    /// Mixture-of-experts models only read part of their weights per token.
    active_gb: Option<f64>,
    quality: u8,
    good_for: Vec<String>,
    note: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Catalog {
    verified_at: String,
    models: Vec<CatalogEntry>,
}

fn catalog() -> &'static Catalog {
    static C: std::sync::OnceLock<Catalog> = std::sync::OnceLock::new();
    C.get_or_init(|| serde_json::from_str(include_str!("../resources/ollama-catalog.json")).expect("catalog is valid"))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    pub name: String,
    pub label: String,
    pub size_gb: f64,
    pub memory_needed_gb: f64,
    pub quality: u8,
    pub good_for: Vec<String>,
    pub note: String,
    /// "fits" | "tight" | "too-big"
    pub fit: String,
    pub est_tokens_per_sec: f64,
    /// Rough time to grade one written answer (~2.5k tokens in, ~350 out). Grading runs in the
    /// background while the learner answers the next question, so ~1.5 minutes is acceptable.
    pub est_grade_seconds: u32,
    pub installed: bool,
    pub recommended: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogView {
    pub system: SystemInfo,
    pub verified_at: String,
    pub models: Vec<CatalogModel>,
}

/// Memory needed ≈ weights + KV cache/runtime overhead for an 8k context.
fn memory_needed(size_gb: f64) -> f64 {
    ((size_gb * 1.1 + 1.0) * 10.0).round() / 10.0
}

/// Effective memory bandwidth (GB/s) — generation speed is roughly bandwidth / bytes read per token.
fn bandwidth(sys: &SystemInfo, threads_fraction: f64) -> f64 {
    match sys.accelerator.as_str() {
        "apple-silicon" => 110.0,
        "nvidia" => 250.0,
        // Calibrated on a 20-core laptop: qwen2.5-coder:1.5b graded an answer in ~17 s at half the cores.
        _ => 40.0 * threads_fraction.clamp(0.3, 1.0) + 15.0,
    }
}

fn estimate(sys: &SystemInfo, e: &CatalogEntry) -> (f64, u32) {
    let read_gb = e.active_gb.unwrap_or(e.size_gb).max(0.3);
    let tps = (bandwidth(sys, 0.5) / read_gb).min(150.0);
    // Prompt processing is compute-bound: much faster than generation on GPUs, ~6-10x on CPUs.
    let prefill = tps * if sys.accelerator == "cpu" { 8.0 } else { 25.0 };
    let secs = 2500.0 / prefill + 350.0 / tps;
    ((tps * 10.0).round() / 10.0, secs.round() as u32)
}

pub fn catalog_for(sys: &SystemInfo, installed: &HashSet<String>) -> CatalogView {
    let mut models: Vec<CatalogModel> = catalog()
        .models
        .iter()
        .map(|e| {
            let need = memory_needed(e.size_gb);
            let fit = if need <= sys.usable_for_models_gb * 0.75 {
                "fits"
            } else if need <= sys.usable_for_models_gb {
                "tight"
            } else {
                "too-big"
            };
            let (tps, grade_s) = estimate(sys, e);
            CatalogModel {
                name: e.name.clone(),
                label: e.label.clone(),
                size_gb: e.size_gb,
                memory_needed_gb: need,
                quality: e.quality,
                good_for: e.good_for.clone(),
                note: e.note.clone(),
                fit: fit.into(),
                est_tokens_per_sec: tps,
                est_grade_seconds: grade_s,
                installed: installed.contains(&e.name) || installed.contains(&format!("{}:latest", e.name)),
                recommended: false,
            }
        })
        .collect();

    // Recommend the best quality that fits comfortably and grades in under ~1.5 minutes.
    let best = models
        .iter()
        .filter(|m| m.fit == "fits" && m.est_grade_seconds <= 100 && m.good_for.iter().any(|g| g == "grading"))
        .map(|m| m.quality)
        .max();
    if let Some(q) = best {
        let mut picks: Vec<&mut CatalogModel> = models
            .iter_mut()
            .filter(|m| m.fit == "fits" && m.est_grade_seconds <= 100 && m.quality == q && m.good_for.iter().any(|g| g == "grading"))
            .collect();
        picks.sort_by_key(|m| m.est_grade_seconds);
        for m in picks.into_iter().take(2) {
            m.recommended = true;
        }
    }
    let fit_rank = |f: &str| match f {
        "fits" => 0,
        "tight" => 1,
        _ => 2,
    };
    models.sort_by(|a, b| {
        b.recommended
            .cmp(&a.recommended)
            .then(fit_rank(&a.fit).cmp(&fit_rank(&b.fit)))
            .then(b.quality.cmp(&a.quality))
            .then(a.size_gb.partial_cmp(&b.size_gb).unwrap_or(std::cmp::Ordering::Equal))
    });
    CatalogView { system: sys.clone(), verified_at: catalog().verified_at.clone(), models }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LookupResult {
    pub name: String,
    pub size_gb: f64,
    pub memory_needed_gb: f64,
    pub fit: String,
}

/// Checks a model name against the Ollama registry and returns its download size.
pub async fn lookup(http: &reqwest::Client, name: &str, sys: &SystemInfo) -> AppResult<LookupResult> {
    let name = name.trim();
    if name.is_empty() || name.contains(char::is_whitespace) {
        return Err(AppError::Invalid("Enter a model name like qwen2.5:7b".into()));
    }
    let (repo, tag) = name.split_once(':').unwrap_or((name, "latest"));
    let repo_path = if repo.contains('/') { repo.to_string() } else { format!("library/{repo}") };
    let url = format!("https://registry.ollama.ai/v2/{repo_path}/manifests/{tag}");
    let resp = http
        .get(url)
        .header("Accept", "application/vnd.docker.distribution.manifest.v2+json")
        .send()
        .await?;
    if resp.status().as_u16() == 404 {
        return Err(AppError::NotFound(format!("“{name}” isn't in the Ollama library. Check the name on ollama.com/library.")));
    }
    if !resp.status().is_success() {
        return Err(AppError::Provider(format!("Ollama registry: {}", resp.status())));
    }
    let v: Value = resp.json().await?;
    let bytes: u64 = v["layers"].as_array().map(|l| l.iter().filter_map(|x| x["size"].as_u64()).sum()).unwrap_or(0);
    let size_gb = (bytes as f64 / 1e9 * 100.0).round() / 100.0;
    let need = memory_needed(size_gb);
    let fit = if need <= sys.usable_for_models_gb * 0.75 {
        "fits"
    } else if need <= sys.usable_for_models_gb {
        "tight"
    } else {
        "too-big"
    };
    Ok(LookupResult { name: name.to_string(), size_gb, memory_needed_gb: need, fit: fit.into() })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullProgress {
    pub status: String,
    pub completed: Option<u64>,
    pub total: Option<u64>,
}

pub async fn pull(
    http: &reqwest::Client,
    base_url: &str,
    name: &str,
    on_progress: &mut (dyn FnMut(PullProgress) + Send),
) -> AppResult<()> {
    // Downloads can take a long time; no read timeout on this request.
    let resp = http
        .post(format!("{base_url}/api/pull"))
        .timeout(std::time::Duration::from_secs(6 * 3600))
        .json(&json!({ "model": name, "stream": true }))
        .send()
        .await
        .map_err(|e| AppError::Network(format!("Ollama isn't reachable at {base_url} ({e})")))?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::Provider(format!("Download failed: {body}")));
    }
    let mut buf = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        buf.extend_from_slice(&chunk?);
        while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let Ok(v) = serde_json::from_slice::<Value>(&line) else { continue };
            if let Some(err) = v["error"].as_str() {
                return Err(AppError::Provider(format!("Download failed: {err}")));
            }
            on_progress(PullProgress {
                status: v["status"].as_str().unwrap_or("").to_string(),
                completed: v["completed"].as_u64(),
                total: v["total"].as_u64(),
            });
        }
    }
    Ok(())
}

pub async fn delete(http: &reqwest::Client, base_url: &str, name: &str) -> AppResult<()> {
    let resp = http.delete(format!("{base_url}/api/delete")).json(&json!({ "model": name })).send().await?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::Provider(format!("Delete failed: {body}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sys(accel: &str, total: f64) -> SystemInfo {
        let usable = if accel == "apple-silicon" { total * 0.7 } else { (total - 4.0).max(total * 0.5) };
        SystemInfo {
            os: "linux".into(),
            arch: "x86_64".into(),
            cpu_cores: 20,
            total_ram_gb: total,
            available_ram_gb: total / 2.0,
            accelerator: accel.into(),
            usable_for_models_gb: usable,
        }
    }

    #[test]
    fn catalog_fits_this_linux_pc() {
        let view = catalog_for(&sys("cpu", 15.0), &HashSet::from(["qwen2.5:7b".to_string()]));
        let get = |n: &str| view.models.iter().find(|m| m.name == n).unwrap();
        assert_eq!(get("qwen2.5:7b").fit, "fits");
        assert!(get("qwen2.5:7b").installed);
        assert_eq!(get("qwen3.5:27b").fit, "too-big");
        assert!(view.models.iter().any(|m| m.recommended));
        assert!(view.models[0].recommended, "recommended models are listed first");
        // MoE models are estimated faster than dense models of similar size
        assert!(get("gpt-oss:20b").est_tokens_per_sec > get("phi4:14b").est_tokens_per_sec);
    }

    #[test]
    fn bigger_mac_fits_more() {
        let view = catalog_for(&sys("apple-silicon", 32.0), &HashSet::new());
        let get = |n: &str| view.models.iter().find(|m| m.name == n).unwrap();
        assert_eq!(get("gpt-oss:20b").fit, "fits");
        assert!(get("qwen2.5:7b").est_tokens_per_sec > 15.0);
    }
}
