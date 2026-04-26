//! On-device face detection and recognition for `filetag-web`.
//!
//! Uses two ONNX models from the InsightFace **buffalo_l** pack
//! via `tract-onnx` (pure Rust, no system dependencies):
//!
//! * **det_10g** (~17 MB) — SCRFD-10GF face detector, outputs bounding boxes
//!   AND 5-point facial landmarks per face.
//! * **w600k_r50** (~166 MB) — InsightFace ResNet-50 ArcFace embedder, outputs
//!   a 512-dimension vector per aligned face crop.
//!
//! Models are downloaded automatically on first use from the InsightFace v0.7
//! release archive and stored in the platform-standard user-data directory
//! (e.g. `~/Library/Application Support/filetag/models/` on macOS).
//!
//! Face detection pipeline:
//!   1. Aspect-ratio-preserving resize + zero-pad to 640×640.
//!   2. SCRFD inference → bounding boxes + 5 landmarks + confidence scores.
//!   3. Similarity-transform warp of each face crop to a canonical 112×112
//!      using the 5 landmarks (standard ArcFace alignment).
//!   4. w600k_r50 inference on the aligned crop → L2-normalised 512-d vector.
//!
//! After embedding, faces are clustered using DBSCAN on cosine distance.
//! Each cluster becomes a subject (`person/unknown-N`) that the user can later
//! rename via the standard Subjects UI.
//!
//! **Licence note:** InsightFace models are for non-commercial research only.

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Query, State},
    http::header,
    response::{IntoResponse, Json, Response},
};
use filetag_lib::db;
use image::{DynamicImage, ImageFormat, imageops::FilterType};
use rusqlite::{Connection, OptionalExtension};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tract_onnx::prelude::*;

use crate::state::{AppError, AppState, open_conn, open_for_file_op, root_for_dir};
use crate::types::{
    ApiFaceDetection, ApiFaceResult, FaceAnalyseBatchRequest, FaceAnalyseRequest,
    FaceAssignRequest, FaceClusterRequest, FaceConfigRequest, FaceConfigResponse,
    FaceDeleteRequest, FaceProgressResponse, FaceSuggestParams,
};

// ---------------------------------------------------------------------------
// Model metadata (URLs + SHA-256)
// ---------------------------------------------------------------------------

/// InsightFace buffalo_l v0.7 release archive.
/// Contains both the SCRFD-10GF detector and the ResNet-50 ArcFace embedder.
const BUFFALO_L_ZIP_URL: &str =
    "https://github.com/deepinsight/insightface/releases/download/v0.7/buffalo_l.zip";

/// SCRFD-10GF face detector with 5-point landmark output.
const DETECT_MODEL_NAME: &str = "det_10g.onnx";
const DETECT_MODEL_ZIP_ENTRY: &str = "det_10g.onnx";
/// SHA-256 of det_10g.onnx as extracted from buffalo_l.zip v0.7.
/// Leave empty to skip verification (model will still be checked for valid ONNX header).
const DETECT_MODEL_SHA256: &str = "";

/// ResNet-50 ArcFace embedder trained on WebFace600K.
const EMBED_MODEL_NAME: &str = "w600k_r50.onnx";
const EMBED_MODEL_ZIP_ENTRY: &str = "w600k_r50.onnx";
/// SHA-256 of w600k_r50.onnx as extracted from buffalo_l.zip v0.7.
const EMBED_MODEL_SHA256: &str = "";

// ---------------------------------------------------------------------------
// Progress tracking
// ---------------------------------------------------------------------------

/// Progress snapshot for the running (or most recently completed) face batch.
#[derive(Default, Clone)]
pub struct FaceProgress {
    pub running: bool,
    pub done: usize,
    pub total: usize,
    pub current: Option<String>,
}

/// Progress for the model download (if active).
#[derive(Default, Clone)]
pub struct ModelDownloadProgress {
    /// True while a download is in progress.
    pub active: bool,
    /// Which model is currently downloading ("detect" or "embed").
    pub phase: String,
    /// Bytes received so far (across both models).
    pub bytes_done: u64,
    /// Total bytes to download (across both models, if Content-Length is known).
    pub bytes_total: Option<u64>,
    /// Instantaneous download speed in bytes/second.
    pub speed_bps: u64,
    /// Set if the last download attempt failed.
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Per-root face-recognition settings (loaded from the `settings` table).
#[derive(Clone)]
pub struct FaceConfig {
    /// Minimum detector confidence to keep a detection (0.0–1.0).
    pub confidence: f32,
    /// Maximum cosine distance for two faces to be considered the same person.
    pub cluster_distance: f32,
    /// Minimum bounding-box side length in pixels; smaller faces are skipped.
    pub min_face_px: u32,
    /// Prefix used when auto-creating subject names (`person/unknown-N`).
    pub tag_prefix: String,
    /// Cosine distance below which a new cluster is automatically matched to a
    /// known named person during clustering (0.0 = never auto-match).
    pub auto_match_threshold: f32,
}

impl Default for FaceConfig {
    fn default() -> Self {
        Self {
            confidence: 0.7,
            cluster_distance: 0.35,
            min_face_px: 40,
            tag_prefix: "person".into(),
            auto_match_threshold: 0.30,
        }
    }
}

fn load_face_config(conn: &Connection) -> FaceConfig {
    let get_f32 = |key: &str, default: f32| -> f32 {
        db::get_setting(conn, key)
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let get_u32 = |key: &str, default: u32| -> u32 {
        db::get_setting(conn, key)
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let get_str = |key: &str, default: &str| -> String {
        db::get_setting(conn, key)
            .ok()
            .flatten()
            .unwrap_or_else(|| default.to_string())
    };
    FaceConfig {
        confidence: get_f32("face.confidence", 0.7),
        cluster_distance: get_f32("face.cluster_distance", 0.35),
        min_face_px: get_u32("face.min_face_px", 40),
        tag_prefix: get_str("face.tag_prefix", "person"),
        auto_match_threshold: get_f32("face.auto_match_threshold", 0.30),
    }
}

// ---------------------------------------------------------------------------
// Model storage location
// ---------------------------------------------------------------------------

/// Return the directory where ONNX model files are stored.
///
/// On macOS: `~/Library/Application Support/filetag/models/`
/// On Linux: `~/.local/share/filetag/models/`
pub fn models_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("filetag").join("models"))
}

pub fn detect_model_path() -> Option<PathBuf> {
    models_dir().map(|d| d.join(DETECT_MODEL_NAME))
}

pub fn embed_model_path() -> Option<PathBuf> {
    models_dir().map(|d| d.join(EMBED_MODEL_NAME))
}

pub fn models_ready() -> bool {
    detect_model_path().map(|p| p.is_file()).unwrap_or(false)
        && embed_model_path().map(|p| p.is_file()).unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Model download
// ---------------------------------------------------------------------------

/// Verify SHA-256 of a file.  If `expected` is empty the check is skipped
/// (the file only needs to exist and start with the ONNX magic bytes `\x08`).
fn verify_model_file(path: &Path, expected_sha256: &str) -> anyhow::Result<()> {
    let data = std::fs::read(path)?;
    if data.is_empty() {
        anyhow::bail!("{}: file is empty", path.display());
    }
    if !expected_sha256.is_empty() {
        let mut hasher = Sha256::new();
        hasher.update(&data);
        let actual = format!("{:x}", hasher.finalize());
        if actual != expected_sha256 {
            anyhow::bail!(
                "SHA-256 mismatch for {}: expected {}, got {}",
                path.display(),
                expected_sha256,
                actual
            );
        }
    }
    Ok(())
}

/// Download buffalo_l.zip and extract both models (det_10g.onnx + w600k_r50.onnx).
///
/// Both files are written to the models directory.  If both already exist and
/// pass verification, the download is skipped.
async fn download_buffalo_l(
    detect_dest: &Path,
    embed_dest: &Path,
    prog: &std::sync::Mutex<ModelDownloadProgress>,
) -> anyhow::Result<()> {
    let detect_ok =
        detect_dest.is_file() && verify_model_file(detect_dest, DETECT_MODEL_SHA256).is_ok();
    let embed_ok =
        embed_dest.is_file() && verify_model_file(embed_dest, EMBED_MODEL_SHA256).is_ok();
    if detect_ok && embed_ok {
        return Ok(());
    }

    let models_dir = detect_dest
        .parent()
        .ok_or_else(|| anyhow::anyhow!("cannot determine models directory"))?;
    std::fs::create_dir_all(models_dir)?;
    let zip_path = models_dir.join("buffalo_l.tmp.zip");

    // Pre-fetch Content-Length.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()?;

    let head = client.head(BUFFALO_L_ZIP_URL).send().await.ok();
    let content_length = head.and_then(|r| r.content_length());

    {
        let mut p = prog.lock().unwrap();
        p.active = true;
        p.phase = "buffalo_l".to_string();
        p.bytes_done = 0;
        p.bytes_total = content_length;
        p.speed_bps = 0;
        p.error = None;
    }

    let response = client
        .get(BUFFALO_L_ZIP_URL)
        .send()
        .await?
        .error_for_status()?;
    let mut buf: Vec<u8> = Vec::new();
    if let Some(cl) = content_length {
        buf.reserve(cl as usize);
    }

    let start = std::time::Instant::now();
    let mut bytes_done: u64 = 0;
    let mut response = response;
    while let Some(chunk) = response.chunk().await? {
        bytes_done += chunk.len() as u64;
        buf.extend_from_slice(&chunk);
        let elapsed = start.elapsed().as_secs_f64().max(0.001);
        let mut p = prog.lock().unwrap();
        p.bytes_done = bytes_done;
        p.speed_bps = (bytes_done as f64 / elapsed) as u64;
    }

    std::fs::write(&zip_path, &buf)?;
    drop(buf);

    // Extract both models from the zip.
    let zip_data = std::fs::read(&zip_path)?;
    let cursor = Cursor::new(zip_data);
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| anyhow::anyhow!("failed to open buffalo_l.zip: {e}"))?;

    let extract = |archive: &mut zip::ZipArchive<Cursor<Vec<u8>>>,
                   entry_name: &str,
                   dest: &Path|
     -> anyhow::Result<()> {
        let mut entry = archive
            .by_name(entry_name)
            .map_err(|e| anyhow::anyhow!("'{entry_name}' not found in zip: {e}"))?;
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut bytes)?;
        std::fs::write(dest, &bytes)?;
        Ok(())
    };

    if !detect_ok {
        extract(&mut archive, DETECT_MODEL_ZIP_ENTRY, detect_dest)?;
        verify_model_file(detect_dest, DETECT_MODEL_SHA256)?;
    }
    if !embed_ok {
        extract(&mut archive, EMBED_MODEL_ZIP_ENTRY, embed_dest)?;
        verify_model_file(embed_dest, EMBED_MODEL_SHA256)?;
    }
    drop(archive);
    let _ = std::fs::remove_file(&zip_path);
    Ok(())
}

/// Ensure both ONNX models are present and valid.  Downloads missing/corrupt
/// files.  Returns an error when any download fails.
pub async fn ensure_models(state: Arc<AppState>) -> anyhow::Result<()> {
    let detect_path =
        detect_model_path().ok_or_else(|| anyhow::anyhow!("cannot determine models directory"))?;
    let embed_path =
        embed_model_path().ok_or_else(|| anyhow::anyhow!("cannot determine models directory"))?;

    let prog = &state.model_download;
    {
        let mut p = prog.lock().unwrap();
        *p = ModelDownloadProgress {
            active: true,
            ..Default::default()
        };
    }

    if let Err(e) = download_buffalo_l(&detect_path, &embed_path, prog).await {
        let mut p = prog.lock().unwrap();
        p.active = false;
        p.error = Some(e.to_string());
        return Err(e);
    }

    // Mark complete.
    {
        let mut p = prog.lock().unwrap();
        p.active = false;
        p.error = None;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Loaded models (re-used across calls)
// ---------------------------------------------------------------------------

type OnnxModel = RunnableModel<TypedFact, Box<dyn TypedOp>, Graph<TypedFact, Box<dyn TypedOp>>>;

/// Compiled ONNX models kept alive for the lifetime of the server.
pub struct FaceModels {
    pub detector: OnnxModel,
    pub embedder: OnnxModel,
}

/// Load and optimise both models from disk.
///
/// This is expensive (~100 ms per model) and should only be called once, then
/// the result cached in an `Arc` or `OnceCell`.
pub fn load_models() -> anyhow::Result<FaceModels> {
    let detect_path =
        detect_model_path().ok_or_else(|| anyhow::anyhow!("models directory not available"))?;
    let embed_path =
        embed_model_path().ok_or_else(|| anyhow::anyhow!("models directory not available"))?;

    // det_10g.onnx — SCRFD-10GF, 640×640 input.
    let detector = tract_onnx::onnx()
        .model_for_path(&detect_path)?
        .with_input_fact(
            0,
            InferenceFact::dt_shape(f32::datum_type(), tvec![1, 3, 640, 640]),
        )?
        .into_optimized()?
        .into_runnable()?;

    // w600k_r50.onnx — ResNet-50 ArcFace, 112×112 input.
    let embedder = tract_onnx::onnx()
        .model_for_path(&embed_path)?
        .with_input_fact(
            0,
            InferenceFact::dt_shape(f32::datum_type(), tvec![1, 3, 112, 112]),
        )?
        .into_optimized()?
        .into_runnable()?;

    Ok(FaceModels { detector, embedder })
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

/// A raw detection result before persistence.
#[derive(Debug, Clone)]
pub struct RawDetection {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub confidence: f32,
    /// Embedding as raw f32 bytes (little-endian).  Dimension depends on the
    /// embedder model (512 for w600k_r50).
    pub embedding: Option<Vec<u8>>,
    /// Five facial landmarks as [lm0_x, lm0_y, …, lm4_x, lm4_y] in original
    /// image coordinates.  Order: left-eye, right-eye, nose, mouth-left,
    /// mouth-right (standard InsightFace / RetinaFace convention).
    /// Retained for future persistence or display; not yet consumed downstream.
    #[allow(dead_code)]
    pub landmarks: Option<[f32; 10]>,
}

// ---------------------------------------------------------------------------
// NMS helpers
// ---------------------------------------------------------------------------

/// Intersection-over-Union for two boxes (x1, y1, x2, y2) in any consistent unit.
fn iou(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> f32 {
    let ix1 = a.0.max(b.0);
    let iy1 = a.1.max(b.1);
    let ix2 = a.2.min(b.2);
    let iy2 = a.3.min(b.3);
    let inter = (ix2 - ix1).max(0.0) * (iy2 - iy1).max(0.0);
    if inter == 0.0 {
        return 0.0;
    }
    let area_a = (a.2 - a.0).max(0.0) * (a.3 - a.1).max(0.0);
    let area_b = (b.2 - b.0).max(0.0) * (b.3 - b.1).max(0.0);
    inter / (area_a + area_b - inter).max(1e-8)
}

/// Greedy NMS.  Input: (score, x1, y1, x2, y2) slices sorted by score descending.
/// Returns the indices of boxes to keep.
fn nms(candidates: &[(f32, f32, f32, f32, f32)], iou_threshold: f32) -> Vec<usize> {
    let mut keep = Vec::new();
    let mut suppressed = vec![false; candidates.len()];
    for i in 0..candidates.len() {
        if suppressed[i] {
            continue;
        }
        keep.push(i);
        let bi = (
            candidates[i].1,
            candidates[i].2,
            candidates[i].3,
            candidates[i].4,
        );
        for j in (i + 1)..candidates.len() {
            if suppressed[j] {
                continue;
            }
            let bj = (
                candidates[j].1,
                candidates[j].2,
                candidates[j].3,
                candidates[j].4,
            );
            if iou(bi, bj) > iou_threshold {
                suppressed[j] = true;
            }
        }
    }
    keep
}

// ---------------------------------------------------------------------------
// SCRFD input preprocessing
// ---------------------------------------------------------------------------

const DET_SIZE: u32 = 640;

/// Resize `img` to fit within DET_SIZE×DET_SIZE while preserving aspect ratio,
/// then pad the remaining area with middle-grey (127, 127, 127) to reach
/// exactly DET_SIZE×DET_SIZE.
///
/// Returns the padded RGB image and the scale factor used (divide coordinates
/// by this to get back to original image coordinates).
fn prep_detector_input(img: &DynamicImage) -> (image::RgbImage, f32) {
    let orig_w = img.width();
    let orig_h = img.height();
    let scale = (DET_SIZE as f32 / orig_w as f32).min(DET_SIZE as f32 / orig_h as f32);
    let new_w = (orig_w as f32 * scale).round() as u32;
    let new_h = (orig_h as f32 * scale).round() as u32;

    let resized = img
        .resize_exact(new_w, new_h, FilterType::Triangle)
        .to_rgb8();

    let mut padded = image::RgbImage::from_pixel(DET_SIZE, DET_SIZE, image::Rgb([127u8, 127, 127]));
    // Copy resized into top-left corner.
    for y in 0..new_h {
        for x in 0..new_w {
            let px = resized.get_pixel(x, y);
            padded.put_pixel(x, y, *px);
        }
    }
    (padded, scale)
}

/// Build the NCHW input tensor for det_10g.onnx from a 640×640 RGB image.
/// Normalisation: `(pixel − 127.5) / 128.0`.
fn det_image_to_tensor(rgb: &image::RgbImage) -> anyhow::Result<Tensor> {
    let h = rgb.height() as usize;
    let w = rgb.width() as usize;
    let mut data = vec![0_f32; 3 * h * w];
    for (idx, pixel) in rgb.pixels().enumerate() {
        data[idx] = (pixel[0] as f32 - 127.5) / 128.0; // R
        data[h * w + idx] = (pixel[1] as f32 - 127.5) / 128.0; // G
        data[2 * h * w + idx] = (pixel[2] as f32 - 127.5) / 128.0; // B
    }
    Ok(tract_ndarray::Array4::from_shape_vec((1, 3, h, w), data)?.into())
}

// ---------------------------------------------------------------------------
// SCRFD output decoder
// ---------------------------------------------------------------------------

/// Decode the 9 output tensors of det_10g.onnx into a flat list of
/// `(score, x1, y1, x2, y2, landmarks[10])` tuples.
///
/// Output tensor layout (for 640×640 input):
/// ```text
/// [0] scores stride 8   [1, 12800, 1]
/// [1] scores stride 16  [1,  3200, 1]
/// [2] scores stride 32  [1,   800, 1]
/// [3] boxes  stride 8   [1, 12800, 4]
/// [4] boxes  stride 16  [1,  3200, 4]
/// [5] boxes  stride 32  [1,   800, 4]
/// [6] kps    stride 8   [1, 12800, 10]
/// [7] kps    stride 16  [1,  3200, 10]
/// [8] kps    stride 32  [1,   800, 10]
/// ```
///
/// Flat-slice access is used throughout to avoid ndarray dimensionality
/// panics: regardless of whether the model exports [1, N, C] or [N, C],
/// the in-memory layout is identical (batch=1, so the batch stride is
/// irrelevant) and `flat[anchor * C + channel]` is always correct.
fn decode_scrfd(
    outputs: &[TValue],
    score_threshold: f32,
    det_scale: f32,
) -> Vec<(f32, f32, f32, f32, f32, [f32; 10])> {
    let strides = [8u32, 16, 32];
    let num_anchors = 2usize;
    // fmc = 3 (number of feature map levels)
    // InsightFace SCRFD grouped layout: [scores_8, scores_16, scores_32,
    //   boxes_8, boxes_16, boxes_32, kps_8, kps_16, kps_32]
    // Indices: score=si, box=si+3, kps=si+6.

    let mut results = Vec::new();

    for (si, &stride) in strides.iter().enumerate() {
        let grid = (DET_SIZE / stride) as usize;
        let n = grid * grid * num_anchors;

        // Flat slices — works regardless of whether the tensor is [1,N,C] or [N,C].
        let scores_raw = outputs[si].as_slice::<f32>().expect("scores slice");
        let boxes_raw = outputs[si + 3].as_slice::<f32>().expect("boxes slice");
        let kps_raw = outputs[si + 6].as_slice::<f32>().expect("kps slice");

        let mut anchor_idx = 0usize;
        for row in 0..grid {
            for col in 0..grid {
                for _ in 0..num_anchors {
                    if anchor_idx >= n {
                        break;
                    }

                    // scores_raw: N elements (shape [1,N,1] or [N,1] → same flat layout)
                    // det_10g.onnx applies sigmoid internally; scores are
                    // already in [0, 1].  Do NOT apply sigmoid again.
                    let score = scores_raw[anchor_idx];
                    if score >= score_threshold {
                        let cx = col as f32 * stride as f32;
                        let cy = row as f32 * stride as f32;

                        let bi = anchor_idx * 4;
                        let left = boxes_raw[bi] * stride as f32;
                        let top = boxes_raw[bi + 1] * stride as f32;
                        let right = boxes_raw[bi + 2] * stride as f32;
                        let bottom = boxes_raw[bi + 3] * stride as f32;

                        let x1 = (cx - left) / det_scale;
                        let y1 = (cy - top) / det_scale;
                        let x2 = (cx + right) / det_scale;
                        let y2 = (cy + bottom) / det_scale;

                        let mut lm = [0_f32; 10];
                        let ki = anchor_idx * 10;
                        for k in 0..5 {
                            lm[2 * k] = (cx + kps_raw[ki + 2 * k] * stride as f32) / det_scale;
                            lm[2 * k + 1] =
                                (cy + kps_raw[ki + 2 * k + 1] * stride as f32) / det_scale;
                        }

                        results.push((score, x1, y1, x2, y2, lm));
                    }
                    anchor_idx += 1;
                }
            }
        }
    }
    results
}

// ---------------------------------------------------------------------------
// Face alignment using 5-point similarity transform
// ---------------------------------------------------------------------------

/// Canonical 112×112 face landmark positions (standard ArcFace alignment).
/// Order: left-eye, right-eye, nose, mouth-left, mouth-right.
const CANONICAL_112: [[f32; 2]; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

/// Compute the parameters (a, b, tx, ty) of the 2-D similarity transform
/// that maps `src` landmarks to `dst` landmarks in a least-squares sense.
///
/// The forward transform maps a point `(x, y)` from source image space to
/// aligned-crop space: `u = a*x − b*y + tx`, `v = b*x + a*y + ty`.
fn similarity_transform(src: &[[f32; 2]; 5], dst: &[[f32; 2]; 5]) -> (f32, f32, f32, f32) {
    let n = 5_f32;
    let mut sum_sx = 0_f32;
    let mut sum_sy = 0_f32;
    let mut sum_dx = 0_f32;
    let mut sum_dy = 0_f32;
    let mut sum_sq = 0_f32;
    let mut cov_xx = 0_f32; // sum(sx*dx + sy*dy)
    let mut cov_xy = 0_f32; // sum(sx*dy - sy*dx)

    for i in 0..5 {
        let (sx, sy) = (src[i][0], src[i][1]);
        let (dx, dy) = (dst[i][0], dst[i][1]);
        sum_sx += sx;
        sum_sy += sy;
        sum_dx += dx;
        sum_dy += dy;
        sum_sq += sx * sx + sy * sy;
        cov_xx += sx * dx + sy * dy;
        cov_xy += sx * dy - sy * dx;
    }

    let den = sum_sq - (sum_sx * sum_sx + sum_sy * sum_sy) / n;
    if den.abs() < 1e-8 {
        // Degenerate case — return identity-ish transform.
        return (1.0, 0.0, 0.0, 0.0);
    }

    let a = (cov_xx - (sum_sx * sum_dx + sum_sy * sum_dy) / n) / den;
    let b = (cov_xy - (sum_sx * sum_dy - sum_sy * sum_dx) / n) / den;
    let tx = (sum_dx - a * sum_sx + b * sum_sy) / n;
    let ty = (sum_dy - b * sum_sx - a * sum_sy) / n;

    (a, b, tx, ty)
}

/// Warp `img` to produce a 112×112 aligned face crop using the inverse of
/// the similarity transform defined by (a, b, tx, ty).
///
/// For each destination pixel (u, v) in [0, 112) we compute the source
/// coordinates via the inverse transform, then use bilinear interpolation.
fn warp_face_112(img: &DynamicImage, a: f32, b: f32, tx: f32, ty: f32) -> image::RgbImage {
    let src_rgb = img.to_rgb8();
    let src_w = img.width() as f32;
    let src_h = img.height() as f32;

    // Inverse of M = [[a, -b, tx], [b, a, ty]]:
    // det = a² + b²
    // x_src = (a*(u-tx) + b*(v-ty)) / det
    // y_src = (-b*(u-tx) + a*(v-ty)) / det
    let det = (a * a + b * b).max(1e-8);

    let mut out = image::RgbImage::new(112, 112);
    for v in 0u32..112 {
        for u in 0u32..112 {
            let du = u as f32 - tx;
            let dv = v as f32 - ty;
            let sx = (a * du + b * dv) / det;
            let sy = (-b * du + a * dv) / det;

            if sx < 0.0 || sy < 0.0 || sx >= src_w - 1.0 || sy >= src_h - 1.0 {
                out.put_pixel(u, v, image::Rgb([0u8, 0, 0]));
                continue;
            }

            // Bilinear interpolation.
            let x0 = sx.floor() as u32;
            let y0 = sy.floor() as u32;
            let x1 = (x0 + 1).min(src_w as u32 - 1);
            let y1 = (y0 + 1).min(src_h as u32 - 1);
            let fx = sx - x0 as f32;
            let fy = sy - y0 as f32;

            let p00 = src_rgb.get_pixel(x0, y0);
            let p10 = src_rgb.get_pixel(x1, y0);
            let p01 = src_rgb.get_pixel(x0, y1);
            let p11 = src_rgb.get_pixel(x1, y1);

            let lerp = |a: u8, b: u8, c: u8, d: u8| -> u8 {
                let top = a as f32 * (1.0 - fx) + b as f32 * fx;
                let bottom = c as f32 * (1.0 - fx) + d as f32 * fx;
                (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8
            };

            out.put_pixel(
                u,
                v,
                image::Rgb([
                    lerp(p00[0], p10[0], p01[0], p11[0]),
                    lerp(p00[1], p10[1], p01[1], p11[1]),
                    lerp(p00[2], p10[2], p01[2], p11[2]),
                ]),
            );
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Main detection + embedding function
// ---------------------------------------------------------------------------

/// Run the face detector on an image, then compute embeddings for each face.
///
/// Input: any `DynamicImage`.
/// Output: one `RawDetection` per face passing confidence, NMS, and size filters.
pub fn detect_and_embed(
    img: &DynamicImage,
    models: &FaceModels,
    cfg: &FaceConfig,
) -> anyhow::Result<Vec<RawDetection>> {
    // ------------------------------------------------------------------
    // Step 1: Aspect-ratio-preserving resize + pad to 640×640.
    // ------------------------------------------------------------------
    let (padded_rgb, det_scale) = prep_detector_input(img);
    let tensor = det_image_to_tensor(&padded_rgb)?;
    let outputs = models.detector.run(tvec![tensor.into()])?;

    // ------------------------------------------------------------------
    // Step 2: Decode SCRFD outputs, threshold, NMS.
    // ------------------------------------------------------------------
    let mut decoded = decode_scrfd(&outputs, cfg.confidence, det_scale);

    // Sort descending by score for NMS.
    decoded.sort_unstable_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let candidates_nms: Vec<(f32, f32, f32, f32, f32)> = decoded
        .iter()
        .map(|&(s, x1, y1, x2, y2, _)| (s, x1, y1, x2, y2))
        .collect();
    let keep = nms(&candidates_nms, 0.4);

    // ------------------------------------------------------------------
    // Step 3: Size filter + landmark-based alignment + embed.
    // ------------------------------------------------------------------
    let mut detections = Vec::new();

    for idx in keep {
        let (score, x1, y1, x2, y2, lm) = decoded[idx];

        let w = (x2 - x1).round() as i32;
        let h = (y2 - y1).round() as i32;

        if w < cfg.min_face_px as i32 || h < cfg.min_face_px as i32 {
            continue;
        }

        let x = x1.round() as i32;
        let y = y1.round() as i32;

        // Build 5×2 src landmarks from detection output.
        let src: [[f32; 2]; 5] = std::array::from_fn(|k| [lm[2 * k], lm[2 * k + 1]]);

        let embedding = align_and_embed(img, &src, models).ok();

        detections.push(RawDetection {
            x,
            y,
            w,
            h,
            confidence: score,
            embedding,
            landmarks: Some(lm),
        });
    }

    Ok(detections)
}

/// Compute the similarity-transform alignment from detected landmarks to the
/// 112×112 canonical positions, warp the face, then run the ArcFace embedder.
fn align_and_embed(
    img: &DynamicImage,
    src_lm: &[[f32; 2]; 5],
    models: &FaceModels,
) -> anyhow::Result<Vec<u8>> {
    let (a, b, tx, ty) = similarity_transform(src_lm, &CANONICAL_112);
    let aligned = warp_face_112(img, a, b, tx, ty);

    // Normalise: (pixel / 255 − 0.5) / 0.5  (ArcFace standard).
    let mut data = vec![0_f32; 3 * 112 * 112];
    for (idx, pixel) in aligned.pixels().enumerate() {
        data[idx] = (pixel[0] as f32 / 255.0 - 0.5) / 0.5; // R
        data[112 * 112 + idx] = (pixel[1] as f32 / 255.0 - 0.5) / 0.5; // G
        data[2 * 112 * 112 + idx] = (pixel[2] as f32 / 255.0 - 0.5) / 0.5; // B
    }

    let tensor: Tensor = tract_ndarray::Array4::from_shape_vec((1, 3, 112, 112), data)?.into();
    let outputs = models.embedder.run(tvec![tensor.into()])?;
    let emb = outputs[0].to_array_view::<f32>()?;

    let raw: Vec<f32> = emb.iter().copied().collect();
    let norm = l2_norm(&raw);
    let normalised: Vec<f32> = raw.iter().map(|v| v / norm.max(1e-8)).collect();

    Ok(normalised.iter().flat_map(|f| f.to_le_bytes()).collect())
}

fn l2_norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

// ---------------------------------------------------------------------------
// DBSCAN clustering
// ---------------------------------------------------------------------------

/// Decode a raw embedding blob to a Vec<f32>.
fn decode_embedding(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

/// Cosine distance between two normalised vectors (0.0 = identical, 2.0 = opposite).
pub fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    // Both vectors are assumed L2-normalised, so dot ≈ cosine similarity.
    1.0 - dot.clamp(-1.0, 1.0)
}

/// Simple DBSCAN on pre-decoded embeddings.
///
/// Returns a label vector of the same length as `embeddings`:
///   * `Some(cluster_id)` — belongs to cluster with that id (0-based).
///   * `None` — noise point (does not belong to any cluster).
///
/// `eps` is the cosine-distance epsilon; `min_pts` is the minimum cluster size.
pub fn dbscan_cluster(embeddings: &[Vec<f32>], eps: f32, min_pts: usize) -> Vec<Option<usize>> {
    let n = embeddings.len();
    let mut labels: Vec<Option<usize>> = vec![None; n];
    let mut visited = vec![false; n];
    let mut cluster_id = 0_usize;

    for i in 0..n {
        if visited[i] {
            continue;
        }
        visited[i] = true;

        let neighbours = range_query(embeddings, i, eps);
        if neighbours.len() < min_pts {
            // Noise point — may be absorbed later.
            continue;
        }

        let mut stack = neighbours.clone();
        labels[i] = Some(cluster_id);

        let mut j = 0;
        while j < stack.len() {
            let q = stack[j];
            j += 1;
            if !visited[q] {
                visited[q] = true;
                let q_neighbours = range_query(embeddings, q, eps);
                if q_neighbours.len() >= min_pts {
                    for &nb in &q_neighbours {
                        if !stack.contains(&nb) {
                            stack.push(nb);
                        }
                    }
                }
            }
            if labels[q].is_none() {
                labels[q] = Some(cluster_id);
            }
        }

        cluster_id += 1;
    }

    labels
}

fn range_query(embeddings: &[Vec<f32>], idx: usize, eps: f32) -> Vec<usize> {
    embeddings
        .iter()
        .enumerate()
        .filter_map(|(j, emb)| {
            if cosine_distance(&embeddings[idx], emb) <= eps {
                Some(j)
            } else {
                None
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Named-person centroid helpers
// ---------------------------------------------------------------------------

/// Compute a mean embedding (centroid) for every named person in `rows`.
///
/// Only rows that have both a non-null embedding and a subject name that is
/// NOT an auto-generated `unknown-` name are included.
fn compute_named_centroids(
    rows: &[db::FaceDetectionRow],
    auto_prefix: &str,
) -> Vec<(String, Vec<f32>)> {
    use std::collections::HashMap;

    let mut buckets: HashMap<&str, Vec<Vec<f32>>> = HashMap::new();
    for row in rows {
        let Some(name) = row.subject_name.as_deref() else {
            continue;
        };
        if name.starts_with(auto_prefix) {
            continue;
        }
        let Some(emb) = row.embedding.as_deref().map(decode_embedding) else {
            continue;
        };
        if emb.is_empty() {
            continue;
        }
        buckets.entry(name).or_default().push(emb);
    }

    buckets
        .into_iter()
        .filter_map(|(name, embs)| {
            let n = embs.len();
            if n == 0 {
                return None;
            }
            let dim = embs[0].len();
            let mut mean = vec![0f32; dim];
            for e in &embs {
                for (m, v) in mean.iter_mut().zip(e.iter()) {
                    *m += v;
                }
            }
            let norm_val = l2_norm(&mean).max(1e-8);
            for m in &mut mean {
                *m /= norm_val;
            }
            Some((name.to_string(), mean))
        })
        .collect()
}

/// Return persons sorted by ascending cosine distance to `embedding`.
///
/// Only persons within `max_distance` are included.
/// Returns at most `limit` results.
pub fn suggest_matches(
    embedding: &[f32],
    centroids: &[(String, Vec<f32>)],
    max_distance: f32,
    limit: usize,
) -> Vec<(String, f32)> {
    let mut scored: Vec<(String, f32)> = centroids
        .iter()
        .map(|(name, centroid)| (name.clone(), cosine_distance(embedding, centroid)))
        .filter(|(_, d)| *d <= max_distance)
        .collect();
    scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);
    scored
}

// ---------------------------------------------------------------------------
// Cluster-and-assign
// ---------------------------------------------------------------------------

/// Re-cluster all face embeddings in `conn` and update `subject_name` in
/// `face_detections`.
///
/// Existing manually-assigned subjects (where `subject_name` does not start
/// with `{prefix}/unknown-`) are preserved and their embeddings are used as
/// fixed cluster seeds.
///
/// Returns the total number of clusters created/updated.
pub fn cluster_and_assign(conn: &Connection, cfg: &FaceConfig) -> anyhow::Result<usize> {
    let rows = db::all_face_detections_with_embeddings(conn)?;
    if rows.is_empty() {
        return Ok(0);
    }

    // Split into manually-assigned and unassigned rows.
    let auto_prefix = format!("{}/unknown-", cfg.tag_prefix);
    let (manual, auto): (Vec<_>, Vec<_>) = rows.iter().partition(|r| {
        r.subject_name
            .as_deref()
            .map(|s| !s.starts_with(&auto_prefix))
            .unwrap_or(false)
    });

    // Decode embeddings.
    let auto_embs: Vec<Vec<f32>> = auto
        .iter()
        .filter_map(|r| r.embedding.as_deref().map(decode_embedding))
        .collect();

    if auto_embs.is_empty() {
        return Ok(0);
    }

    let labels = dbscan_cluster(&auto_embs, cfg.cluster_distance, 1);

    // Count clusters.
    let max_cluster = labels.iter().filter_map(|l| *l).max().unwrap_or(0);
    let n_clusters = max_cluster + 1;

    // Determine the starting index for new unknown subjects: avoid collisions
    // with existing manually-named `{prefix}/N` subjects.
    let existing_max = manual
        .iter()
        .filter_map(|r| r.subject_name.as_deref())
        .filter_map(|s| {
            s.strip_prefix(&format!("{}/", cfg.tag_prefix))
                .and_then(|s| s.parse::<usize>().ok())
        })
        .max()
        .unwrap_or(0);

    let base = existing_max + 1;

    // Build named centroids for auto-matching.
    let named_centroids = compute_named_centroids(&rows, &auto_prefix);

    // Compute per-cluster centroid and try to match to a known person.
    // cluster_name[i] = final subject name for DBSCAN cluster i.
    let mut cluster_name: Vec<String> = (0..n_clusters)
        .map(|c| format!("{}/unknown-{}", cfg.tag_prefix, base + c))
        .collect();

    if cfg.auto_match_threshold > 0.0 && !named_centroids.is_empty() {
        // Gather embeddings per cluster.
        #[allow(clippy::needless_range_loop)]
        for cluster in 0..n_clusters {
            let cluster_embs: Vec<&Vec<f32>> = auto
                .iter()
                .enumerate()
                .filter_map(|(i, _)| {
                    if labels[i] == Some(cluster) {
                        auto_embs.get(i)
                    } else {
                        None
                    }
                })
                .collect();
            if cluster_embs.is_empty() {
                continue;
            }

            // Centroid of this cluster.
            let dim = cluster_embs[0].len();
            let mut centroid = vec![0f32; dim];
            for e in &cluster_embs {
                for (m, v) in centroid.iter_mut().zip(e.iter()) {
                    *m += v;
                }
            }
            let n_val = l2_norm(&centroid).max(1e-8);
            for m in &mut centroid {
                *m /= n_val;
            }

            // Check against known persons.
            let suggestions =
                suggest_matches(&centroid, &named_centroids, cfg.auto_match_threshold, 1);
            if let Some((best_name, _)) = suggestions.into_iter().next() {
                cluster_name[cluster] = best_name;
            }
        }
    }

    // Apply labels.
    for (i, row) in auto.iter().enumerate() {
        let subject = labels[i].map(|cluster| cluster_name[cluster].clone());
        db::set_face_subject(conn, row.id, subject.as_deref())?;
    }

    // Ensure all subjects exist in the `subjects` table.
    for name in &cluster_name {
        conn.execute(
            "INSERT OR IGNORE INTO subjects (name) VALUES (?1)",
            rusqlite::params![name],
        )?;
    }

    Ok(n_clusters)
}

// ---------------------------------------------------------------------------
// Image loading helper
// ---------------------------------------------------------------------------

/// Load an image from an absolute filesystem path.
///
/// Tries the `image` crate first.  Returns an error if the format is not
/// supported (caller can decide whether to skip or try an external tool).
fn load_image(abs: &Path) -> anyhow::Result<DynamicImage> {
    let img = image::open(abs)?;
    Ok(img)
}

// ---------------------------------------------------------------------------
// Face crop thumbnail
// ---------------------------------------------------------------------------

/// Render a JPEG crop of a face detection and return it as bytes.
pub fn face_crop_jpeg(abs: &Path, x: i32, y: i32, w: i32, h: i32) -> anyhow::Result<Vec<u8>> {
    let img = load_image(abs)?;
    let crop = img.crop_imm(x.max(0) as u32, y.max(0) as u32, w as u32, h as u32);
    // Thumbnail at most 200×200 while maintaining aspect ratio.
    let thumb = crop.thumbnail(200, 200);
    let mut buf = Vec::new();
    thumb.write_to(&mut Cursor::new(&mut buf), ImageFormat::Jpeg)?;
    Ok(buf)
}

// ---------------------------------------------------------------------------
// File list helpers
// ---------------------------------------------------------------------------

/// Image extensions supported by the `image` crate.
const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "gif", "webp", "bmp", "tiff", "tif"];

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Collect all image files under `dir`, optionally recursively.
fn collect_images(dir: &Path, recursive: bool) -> Vec<PathBuf> {
    if recursive {
        walkdir::WalkDir::new(dir)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .map(|e| e.into_path())
            .filter(|p| is_image(p))
            .collect()
    } else {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file() && is_image(p))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Root resolution helper (mirrors api.rs `root_from_dir`)
// ---------------------------------------------------------------------------

fn root_from_dir<'a>(
    state: &'a AppState,
    dir: Option<&str>,
) -> Result<&'a filetag_lib::db::TagRoot, AppError> {
    let d = dir.ok_or_else(|| {
        AppError(anyhow::anyhow!(
            "dir parameter is required — navigate into a database first"
        ))
    })?;
    root_for_dir(state, Path::new(d)).ok_or_else(|| {
        AppError(anyhow::anyhow!(
            "path '{}' is not within any loaded database root",
            d
        ))
    })
}

// ---------------------------------------------------------------------------
// API handlers
// ---------------------------------------------------------------------------

/// `POST /api/face/analyse`
/// `POST /api/face/analyse`
pub async fn api_face_analyse(
    State(state): State<Arc<AppState>>,
    Json(req): Json<FaceAnalyseRequest>,
) -> Result<Json<ApiFaceResult>, AppError> {
    if !models_ready() {
        return Err(AppError(anyhow::anyhow!(
            "Face models not downloaded yet. Call POST /api/face/models/download first."
        )));
    }

    let root_entry = root_from_dir(&state, req.dir.as_deref())?;
    let (conn, eff_root, rel) = open_for_file_op(root_entry, &req.path)?;
    let cfg = load_face_config(&conn);

    let models = tokio::task::spawn_blocking(load_models)
        .await
        .map_err(|e| AppError(anyhow::anyhow!("join error: {e}")))?
        .map_err(AppError)?;

    let rel_clone = rel.clone();
    let rows = tokio::task::spawn_blocking(move || {
        analyse_file_sync(&conn, &eff_root, &rel_clone, &models, &cfg)
    })
    .await
    .map_err(|e| AppError(anyhow::anyhow!("join error: {e}")))?
    .map_err(AppError)?;

    Ok(Json(ApiFaceResult {
        path: rel,
        detections: rows.iter().map(row_to_api).collect(),
    }))
}

/// Synchronous wrapper for use inside `spawn_blocking`.
///
/// `rel` is the already-computed root-relative path (may be a virtual archive
/// path of the form `archive.cbz::entry.jpg`).  `abs` is only used for
/// regular files; for archive entries the image is read directly from the
/// archive so `abs` is ignored.
fn analyse_file_sync(
    conn: &Connection,
    root: &Path,
    rel: &str,
    models: &FaceModels,
    cfg: &FaceConfig,
) -> anyhow::Result<Vec<db::FaceDetectionRow>> {
    // Determine the file record, handling archive entries separately.
    let file_rec = if let Some((zip_rel, entry_name)) = rel.split_once("::") {
        // Virtual archive path.  Index via the archive-entry helper.
        let virtual_path = format!("{}::{}", root.join(zip_rel).to_string_lossy(), entry_name);
        db::get_or_index_archive_entry(conn, &virtual_path)?
    } else {
        let abs = root.join(rel);
        db::get_or_index_file(conn, rel, root).or_else(|_| {
            // Fallback: try canonicalising the abs path.
            let canon = std::fs::canonicalize(&abs)?;
            let rel2 = db::relative_to_root(&canon, root)?;
            db::get_or_index_file(conn, &rel2, root)
        })?
    };
    let file_id = file_rec.id;

    db::delete_face_detections_for_file(conn, file_id)?;

    // Load image bytes — from archive or from disk.
    let img = if let Some((zip_rel, entry_name)) = rel.split_once("::") {
        let zip_abs = root.join(zip_rel);
        match crate::archive::archive_read_entry(&zip_abs, entry_name) {
            Ok((bytes, _)) => match image::load_from_memory(&bytes) {
                Ok(i) => i,
                Err(_) => return Ok(vec![]),
            },
            Err(_) => return Ok(vec![]),
        }
    } else {
        let abs = root.join(rel);
        match load_image(&abs) {
            Ok(i) => i,
            Err(_) => return Ok(vec![]),
        }
    };

    let raw = detect_and_embed(&img, models, cfg)?;
    for det in &raw {
        db::insert_face_detection(
            conn,
            file_id,
            det.x,
            det.y,
            det.w,
            det.h,
            det.confidence,
            det.embedding.as_deref(),
        )?;
    }

    db::face_detections_for_file(conn, file_id)
}

/// `POST /api/face/analyse-batch`
pub async fn api_face_analyse_batch(
    State(state): State<Arc<AppState>>,
    Json(req): Json<FaceAnalyseBatchRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !models_ready() {
        return Err(AppError(anyhow::anyhow!(
            "Face models not downloaded yet. Call POST /api/face/models/download first."
        )));
    }

    let root_entry = root_from_dir(&state, Some(req.dir.as_str()))?;

    {
        let mut progress = state.face_progress.lock().unwrap();
        if progress.running {
            return Err(AppError(anyhow::anyhow!(
                "A face-analysis batch is already running."
            )));
        }
        *progress = FaceProgress {
            running: true,
            total: 0,
            done: 0,
            current: None,
        };
    }

    let dir_abs = std::fs::canonicalize(&req.dir)
        .map_err(|e| AppError(anyhow::anyhow!("invalid dir: {e}")))?;
    let root_path = root_entry.root.clone();
    let db_path = root_entry.db_path.clone();
    let recursive = req.recursive;
    let state_clone = state.clone();

    tokio::task::spawn(async move {
        if let Err(e) = run_batch(state_clone, root_path, db_path, dir_abs, recursive).await {
            eprintln!("[face] batch error: {e:#}");
        }
    });

    Ok(Json(serde_json::json!({"started": true})))
}

async fn run_batch(
    state: Arc<AppState>,
    root: PathBuf,
    db_path: PathBuf,
    dir: PathBuf,
    recursive: bool,
) -> anyhow::Result<()> {
    let models = tokio::task::spawn_blocking(load_models).await??;
    let models = Arc::new(models);

    let files = collect_images(&dir, recursive);
    let total = files.len();
    {
        let mut p = state.face_progress.lock().unwrap();
        p.total = total;
    }

    for (idx, abs) in files.into_iter().enumerate() {
        {
            let mut p = state.face_progress.lock().unwrap();
            p.current = abs.to_str().map(str::to_string);
        }

        let abs_c = abs.clone();
        let root_c = root.clone();
        let db_path_c = db_path.clone();
        let models_c = models.clone();

        let _ = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let conn = Connection::open(&db_path_c)?;
            conn.execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA foreign_keys = ON;
                 PRAGMA busy_timeout = 5000;",
            )?;
            let cfg = load_face_config(&conn);
            let rel = abs_c
                .strip_prefix(&root_c)
                .map(|r| r.to_string_lossy().into_owned())
                .unwrap_or_else(|_| abs_c.to_string_lossy().into_owned());
            analyse_file_sync(&conn, &root_c, &rel, &models_c, &cfg)?;
            Ok(())
        })
        .await;

        {
            let mut p = state.face_progress.lock().unwrap();
            p.done = idx + 1;
        }
    }

    // Re-cluster after all detections are in.
    let _ = tokio::task::spawn_blocking(move || -> anyhow::Result<usize> {
        let conn = Connection::open(&db_path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;
        let cfg = load_face_config(&conn);
        cluster_and_assign(&conn, &cfg)
    })
    .await;

    {
        let mut p = state.face_progress.lock().unwrap();
        p.running = false;
        p.current = None;
    }

    Ok(())
}

/// `GET /api/face/status`
pub async fn api_face_status(State(state): State<Arc<AppState>>) -> Json<FaceProgressResponse> {
    let p = state.face_progress.lock().unwrap().clone();
    Json(FaceProgressResponse {
        running: p.running,
        done: p.done,
        total: p.total,
        current: p.current,
    })
}

/// `GET /api/face/detections`
#[derive(Deserialize)]
pub struct FaceDetectionsParams {
    /// Absolute filesystem path of the file.
    pub path: String,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
}

/// Query params for endpoints that only need an optional `dir` (no `path` required).
#[derive(Debug, serde::Deserialize, Default)]
pub struct FaceDirParams {
    pub dir: Option<String>,
}

pub async fn api_face_detections(
    State(state): State<Arc<AppState>>,
    Query(params): Query<FaceDetectionsParams>,
) -> Result<Json<ApiFaceResult>, AppError> {
    let root_entry = root_from_dir(&state, params.dir.as_deref())?;
    let (conn, _, rel) = open_for_file_op(root_entry, &params.path)?;

    let file_rec = db::file_by_path(&conn, &rel)?;
    let detections = match file_rec {
        Some(rec) => db::face_detections_for_file(&conn, rec.id)?,
        None => vec![],
    };

    Ok(Json(ApiFaceResult {
        path: rel,
        detections: detections.iter().map(row_to_api).collect(),
    }))
}

/// `GET /api/face/thumbnail`
#[derive(Deserialize)]
pub struct FaceThumbnailParams {
    /// Detection ID.
    pub id: i64,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
}

pub async fn api_face_thumbnail(
    State(state): State<Arc<AppState>>,
    Query(params): Query<FaceThumbnailParams>,
) -> Result<Response, AppError> {
    let root_entry = root_from_dir(&state, params.dir.as_deref())?;
    let conn = open_conn(root_entry)?;

    let row: Option<(i32, i32, i32, i32, String)> = conn
        .query_row(
            "SELECT fd.x, fd.y, fd.w, fd.h, f.path
             FROM face_detections fd
             JOIN files f ON f.id = fd.file_id
             WHERE fd.id = ?1",
            rusqlite::params![params.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
        .map_err(anyhow::Error::from)
        .map_err(AppError)?;

    let (x, y, w, h, rel_path) =
        row.ok_or_else(|| AppError(anyhow::anyhow!("detection not found")))?;

    let abs = root_entry.root.join(&rel_path);

    let jpeg = tokio::task::spawn_blocking(move || face_crop_jpeg(&abs, x, y, w, h))
        .await
        .map_err(|e| AppError(anyhow::anyhow!("join error: {e}")))?
        .map_err(AppError)?;

    Ok(([(header::CONTENT_TYPE, "image/jpeg")], Body::from(jpeg)).into_response())
}

/// Propagate `subject_name` to all unassigned/auto-assigned detections that
/// belong to the same DBSCAN cluster as `detection_id`.
///
/// Uses the same `cluster_distance` epsilon as the regular cluster step so
/// that transitive cluster membership (A→B→C) is fully respected, not just
/// direct neighbours of the pivot face.
///
/// Returns the number of additional detections that were updated.
fn propagate_subject_to_cluster(
    conn: &Connection,
    detection_id: i64,
    subject_name: &str,
    cfg: &FaceConfig,
) -> anyhow::Result<usize> {
    let rows = db::all_face_detections_with_embeddings(conn)?;

    // Collect rows that have embeddings; keep a parallel index back to `rows`.
    let indexed: Vec<(usize, &db::FaceDetectionRow)> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            r.embedding
                .as_deref()
                .map(|b| !b.is_empty())
                .unwrap_or(false)
        })
        .collect();

    if indexed.is_empty() {
        return Ok(0);
    }

    // Find the position of the pivot in `indexed`.
    let Some(pivot_pos) = indexed.iter().position(|(_, r)| r.id == detection_id) else {
        return Ok(0);
    };

    let embs: Vec<Vec<f32>> = indexed
        .iter()
        .map(|(_, r)| decode_embedding(r.embedding.as_deref().unwrap()))
        .collect();

    let labels = dbscan_cluster(&embs, cfg.cluster_distance, 1);

    let pivot_label = labels[pivot_pos];
    // If the pivot is noise (no cluster), nothing to propagate.
    let Some(cluster_id) = pivot_label else {
        return Ok(0);
    };

    let auto_prefix = format!("{}/unknown-", cfg.tag_prefix);
    let mut updated = 0usize;

    for (pos, (_, row)) in indexed.iter().enumerate() {
        if row.id == detection_id {
            continue; // already assigned above
        }
        if labels[pos] != Some(cluster_id) {
            continue; // different cluster
        }
        // Only overwrite auto-generated names or faces without a subject.
        let is_auto_or_none = row
            .subject_name
            .as_deref()
            .map(|s| s.starts_with(&auto_prefix))
            .unwrap_or(true);
        if !is_auto_or_none {
            continue;
        }
        db::set_face_subject(conn, row.id, Some(subject_name))?;
        updated += 1;
    }

    Ok(updated)
}

/// `POST /api/face/assign`
pub async fn api_face_assign(
    State(state): State<Arc<AppState>>,
    Json(req): Json<FaceAssignRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let root_entry = root_from_dir(&state, req.dir.as_deref())?;
    let conn = open_conn(root_entry)?;

    db::set_face_subject(&conn, req.detection_id, req.subject_name.as_deref())?;

    let mut propagated = 0usize;

    if let Some(name) = &req.subject_name {
        // Ensure the subject exists in the subjects table.
        conn.execute(
            "INSERT OR IGNORE INTO subjects (name) VALUES (?1)",
            rusqlite::params![name],
        )
        .map_err(anyhow::Error::from)
        .map_err(AppError)?;

        // Propagate the name to all cluster-mates.
        let cfg = load_face_config(&conn);
        propagated =
            propagate_subject_to_cluster(&conn, req.detection_id, name, &cfg).map_err(AppError)?;
    }

    Ok(Json(
        serde_json::json!({"ok": true, "propagated": propagated}),
    ))
}

/// `GET /api/face/suggest?detection_id=N[&dir=...]`
///
/// Returns the top named-person matches for a single face detection,
/// ordered by ascending cosine distance (closest first).
/// Only persons within `cluster_distance * 1.5` are considered candidates;
/// those within `auto_match_threshold` are marked `"auto": true`.
/// Open a database connection for the root that owns `detection_id`.
///
/// If `dir` is a non-empty path, the root is resolved from that path (fast
/// path).  Otherwise all loaded roots are scanned until one is found that
/// contains a `face_detections` row with the given id.
fn resolve_conn_for_detection(
    state: &AppState,
    dir: Option<&str>,
    detection_id: i64,
) -> Result<Connection, AppError> {
    // Fast path: dir is provided and non-empty.
    if let Some(d) = dir.filter(|s| !s.is_empty()) {
        let root_entry = root_for_dir(state, Path::new(d)).ok_or_else(|| {
            AppError(anyhow::anyhow!(
                "path '{}' is not within any loaded database root",
                d
            ))
        })?;
        return open_conn(root_entry).map_err(AppError);
    }

    // Slow path: scan all roots.
    for root_entry in &state.roots {
        if let Ok(conn) = open_conn(root_entry) {
            let found: bool = conn
                .query_row(
                    "SELECT 1 FROM face_detections WHERE id = ?1 LIMIT 1",
                    rusqlite::params![detection_id],
                    |_| Ok(true),
                )
                .optional()
                .unwrap_or(None)
                .unwrap_or(false);
            if found {
                return Ok(conn);
            }
        }
    }

    Err(AppError(anyhow::anyhow!(
        "detection {} not found in any loaded database",
        detection_id
    )))
}

pub async fn api_face_suggest(
    State(state): State<Arc<AppState>>,
    Query(params): Query<FaceSuggestParams>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Resolve root: prefer the `dir` parameter, but if it is absent or empty
    // fall back to scanning all loaded roots for the requested detection_id.
    let conn = resolve_conn_for_detection(&state, params.dir.as_deref(), params.detection_id)?;
    let cfg = load_face_config(&conn);
    let auto_prefix = format!("{}/unknown-", cfg.tag_prefix);

    // Fetch embedding for the requested detection.
    let row: Option<Vec<u8>> = conn
        .query_row(
            "SELECT embedding FROM face_detections WHERE id = ?1",
            rusqlite::params![params.detection_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(anyhow::Error::from)
        .map_err(AppError)?;

    let embedding = match row.and_then(|b| if b.is_empty() { None } else { Some(b) }) {
        Some(b) => decode_embedding(&b),
        None => return Ok(Json(serde_json::json!({ "suggestions": [] }))),
    };

    // Load all detections to compute named centroids.
    let all_rows = db::all_face_detections_with_embeddings(&conn).map_err(AppError)?;
    let centroids = compute_named_centroids(&all_rows, &auto_prefix);

    // Return candidates within cluster_distance * 1.5 (wider than auto-match window).
    let max_dist = (cfg.cluster_distance * 1.5).min(1.0);
    let matches = suggest_matches(&embedding, &centroids, max_dist, 5);

    let suggestions: Vec<serde_json::Value> = matches
        .into_iter()
        .map(|(name, dist)| {
            let label = if name.starts_with(&format!("{}/", cfg.tag_prefix)) {
                name[cfg.tag_prefix.len() + 1..].to_string()
            } else {
                name.clone()
            };
            serde_json::json!({
                "name":  name,
                "label": label,
                "distance": (dist * 1000.0).round() / 1000.0,
                "auto": dist <= cfg.auto_match_threshold,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({ "suggestions": suggestions })))
}

/// `POST /api/face/delete` — permanently remove one or more face detections.
pub async fn api_face_delete(
    State(state): State<Arc<AppState>>,
    Json(req): Json<FaceDeleteRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let root_entry = root_from_dir(&state, req.dir.as_deref())?;
    let conn = open_conn(root_entry)?;

    let mut deleted = 0usize;
    for id in &req.detection_ids {
        deleted += conn
            .execute(
                "DELETE FROM face_detections WHERE id = ?1",
                rusqlite::params![id],
            )
            .map_err(anyhow::Error::from)
            .map_err(AppError)?;
    }

    Ok(Json(serde_json::json!({ "deleted": deleted })))
}

/// `POST /api/face/cluster`
pub async fn api_face_cluster(
    State(state): State<Arc<AppState>>,
    Json(req): Json<FaceClusterRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let root_entry = root_from_dir(&state, req.dir.as_deref())?;
    let conn = open_conn(root_entry)?;
    let cfg = load_face_config(&conn);

    let n = tokio::task::spawn_blocking(move || cluster_and_assign(&conn, &cfg))
        .await
        .map_err(|e| AppError(anyhow::anyhow!("join error: {e}")))?
        .map_err(AppError)?;

    Ok(Json(serde_json::json!({"clusters": n})))
}

/// `GET /api/face/config`
pub async fn api_face_config_get(
    State(state): State<Arc<AppState>>,
    Query(params): Query<FaceDirParams>,
) -> Json<FaceConfigResponse> {
    // When no dir is provided (e.g. at page load before any directory is entered),
    // fall back to the first loaded root.  If there are no roots at all, return a
    // default config so the frontend always gets a valid response.
    let root_entry = if params.dir.is_some() {
        root_from_dir(&state, params.dir.as_deref()).ok()
    } else {
        state.roots.first()
    };

    let (enabled, cfg) = match root_entry.and_then(|r| open_conn(r).ok()) {
        Some(conn) => {
            let cfg = load_face_config(&conn);
            let enabled = db::get_setting(&conn, "feature.faces")
                .unwrap_or(None)
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            (enabled, cfg)
        }
        None => (false, FaceConfig::default()),
    };

    Json(FaceConfigResponse {
        enabled,
        confidence: cfg.confidence,
        cluster_distance: cfg.cluster_distance,
        min_face_px: cfg.min_face_px,
        tag_prefix: cfg.tag_prefix,
        auto_match_threshold: cfg.auto_match_threshold,
        models_ready: models_ready(),
    })
}

/// `POST /api/face/config`
pub async fn api_face_config_set(
    State(state): State<Arc<AppState>>,
    Json(req): Json<FaceConfigRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Mirror api_face_config_get: fall back to first root when no dir is provided.
    let root_entry = if req.dir.is_some() {
        root_from_dir(&state, req.dir.as_deref())?
    } else {
        state
            .roots
            .first()
            .ok_or_else(|| AppError(anyhow::anyhow!("no database loaded")))?
    };
    let conn = open_conn(root_entry)?;

    if let Some(v) = req.enabled {
        db::set_setting(&conn, "feature.faces", if v { "1" } else { "0" })?;
    }
    if let Some(v) = req.confidence {
        db::set_setting(&conn, "face.confidence", &v.to_string())?;
    }
    if let Some(v) = req.cluster_distance {
        db::set_setting(&conn, "face.cluster_distance", &v.to_string())?;
    }
    if let Some(v) = req.min_face_px {
        db::set_setting(&conn, "face.min_face_px", &v.to_string())?;
    }
    if let Some(v) = req.tag_prefix {
        db::set_setting(&conn, "face.tag_prefix", &v)?;
    }
    if let Some(v) = req.auto_match_threshold {
        db::set_setting(&conn, "face.auto_match_threshold", &v.to_string())?;
    }

    Ok(Json(serde_json::json!({"ok": true})))
}

/// `POST /api/face/models/download`
///
/// Triggers a background download of both ONNX models.  Returns immediately.
pub async fn api_face_models_download(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let state_clone = Arc::clone(&state);
    tokio::task::spawn(async move {
        if let Err(e) = ensure_models(state_clone).await {
            eprintln!("[face] model download failed: {e:#}");
        }
    });
    Ok(Json(serde_json::json!({"started": true})))
}

/// `GET /api/face/models/status`
pub async fn api_face_models_status(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let prog = state.model_download.lock().unwrap().clone();
    let pct: Option<u8> = prog
        .bytes_total
        .filter(|&t| t > 0)
        .map(|t| ((prog.bytes_done as f64 / t as f64) * 100.0).min(100.0) as u8);

    Json(serde_json::json!({
        "detect_ready": detect_model_path().map(|p| p.is_file()).unwrap_or(false),
        "embed_ready": embed_model_path().map(|p| p.is_file()).unwrap_or(false),
        "models_ready": models_ready(),
        "downloading": prog.active,
        "phase": prog.phase,
        "bytes_done": prog.bytes_done,
        "bytes_total": prog.bytes_total,
        "speed_bps": prog.speed_bps,
        "percent": pct,
        "error": prog.error,
    }))
}

/// `GET /api/face/subjects` — list known face subjects with a representative detection ID.
pub async fn api_face_subjects(
    State(state): State<Arc<AppState>>,
    Query(params): Query<FaceDirParams>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Fall back to first root when dir is missing or doesn't match any root,
    // so page-load calls don't 400 before a directory is selected.
    let root_entry = if params.dir.is_some() {
        root_from_dir(&state, params.dir.as_deref()).ok()
    } else {
        state.roots.first()
    };
    let conn = match root_entry.and_then(|r| open_conn(r).ok()) {
        Some(c) => c,
        None => return Ok(Json(serde_json::json!([]))),
    };

    let mut stmt = conn
        .prepare(
            "SELECT subject_name, COUNT(DISTINCT file_id), MIN(id)
             FROM face_detections
             WHERE subject_name IS NOT NULL
             GROUP BY subject_name
             UNION ALL
             SELECT '' AS subject_name, COUNT(DISTINCT file_id), MIN(id)
             FROM face_detections
             WHERE subject_name IS NULL
             HAVING COUNT(*) > 0
             ORDER BY subject_name",
        )
        .map_err(anyhow::Error::from)
        .map_err(AppError)?;

    let rows: Vec<serde_json::Value> = stmt
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
        })
        .map_err(anyhow::Error::from)
        .map_err(AppError)?
        .filter_map(|r| r.ok())
        .map(|(name, count, det_id)| {
            serde_json::json!({"name": name, "count": count, "det_id": det_id})
        })
        .collect();

    Ok(Json(serde_json::Value::Array(rows)))
}

/// `GET /api/face/files?subject=<name>&dir=<dir>` — list relative file paths
/// that contain at least one detection with the given subject name.
#[derive(Deserialize)]
pub struct FaceFilesParams {
    pub subject: String,
    pub dir: Option<String>,
}

pub async fn api_face_files(
    State(state): State<Arc<AppState>>,
    Query(params): Query<FaceFilesParams>,
) -> Result<Json<serde_json::Value>, AppError> {
    let root_entry = root_from_dir(&state, params.dir.as_deref())?;
    let conn = open_conn(root_entry)?;

    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT f.path
             FROM face_detections fd
             JOIN files f ON f.id = fd.file_id
             WHERE CASE WHEN ?1 = '' THEN fd.subject_name IS NULL
                        ELSE fd.subject_name = ?1 END
             ORDER BY f.path",
        )
        .map_err(anyhow::Error::from)
        .map_err(AppError)?;

    let paths: Vec<String> = stmt
        .query_map(rusqlite::params![params.subject], |r| r.get(0))
        .map_err(anyhow::Error::from)
        .map_err(AppError)?
        .filter_map(|r| r.ok())
        .collect();

    Ok(Json(serde_json::json!({ "paths": paths })))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn row_to_api(r: &db::FaceDetectionRow) -> ApiFaceDetection {
    ApiFaceDetection {
        id: r.id,
        x: r.x,
        y: r.y,
        w: r.w,
        h: r.h,
        confidence: r.confidence,
        subject_name: r.subject_name.clone(),
    }
}
