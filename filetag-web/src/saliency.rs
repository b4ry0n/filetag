//! YOLOv8-based salient region detection for smart thumbnail cropping.
//!
//! Two optional ONNX models:
//!
//! * **yolov8n-pose** (~6.7 MB) — human pose detection with 17 body keypoints.
//!   Default model when AI thumbnail cropping is enabled (`feature.saliency_pose`).
//!   Returns the nose/head position as the salient crop centre.
//!
//! * **yolov8n** (~6.4 MB) — general object detection, 80 COCO classes.
//!   Optional secondary model (`feature.saliency_object`) for non-person images
//!   (animals, vehicles, etc.).
//!
//! Both models are downloaded on demand and cached in the same platform-standard
//! models directory used by `face.rs`.
//!
//! **Source:** Ultralytics YOLOv8 pre-exported ONNX models.
//! Licenced under AGPL-3.0 for non-commercial use.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Context;
use axum::{
    extract::State,
    response::{IntoResponse, Json},
};
use image::DynamicImage;
use serde::Serialize;
use std::sync::Arc;
use tract_onnx::prelude::*;

use crate::face::models_dir;
use crate::state::{AppError, AppState};

// ---------------------------------------------------------------------------
// Model metadata
// ---------------------------------------------------------------------------

const POSE_MODEL_NAME: &str = "yolov8n-pose.onnx";
const POSE_MODEL_URL: &str =
    "https://github.com/ultralytics/assets/releases/download/v0.0.0/yolov8n-pose.onnx";

const OBJECT_MODEL_NAME: &str = "yolov8n.onnx";
const OBJECT_MODEL_URL: &str =
    "https://github.com/ultralytics/assets/releases/download/v0.0.0/yolov8n.onnx";

/// Detection confidence threshold (0–1).  Lower = more detections but
/// more false positives.
const CONF_THRESHOLD: f32 = 0.25;

/// Keypoint visibility threshold.  Below this value a keypoint is
/// considered unreliable and the bbox centre is used instead.
const KP_VIS_THRESHOLD: f32 = 0.3;

/// Input size for both YOLO models.
const YOLO_SIZE: u32 = 640;

// ---------------------------------------------------------------------------
// Download progress
// ---------------------------------------------------------------------------

/// Progress for ongoing saliency model downloads.
#[derive(Default, Clone, Serialize)]
pub struct SaliencyDownloadProgress {
    pub pose_active: bool,
    pub pose_bytes_done: u64,
    pub pose_bytes_total: Option<u64>,
    pub object_active: bool,
    pub object_bytes_done: u64,
    pub object_bytes_total: Option<u64>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Salient point result
// ---------------------------------------------------------------------------

/// A salient focus point in normalised image coordinates ([0, 1] × [0, 1]).
#[derive(Debug, Clone, Copy)]
pub struct SalientPoint {
    /// Horizontal centre, 0 = left, 1 = right.
    pub cx: f32,
    /// Vertical centre, 0 = top, 1 = bottom.
    pub cy: f32,
}

// ---------------------------------------------------------------------------
// Model paths
// ---------------------------------------------------------------------------

pub fn pose_model_path() -> Option<PathBuf> {
    models_dir().map(|d| d.join(POSE_MODEL_NAME))
}

pub fn object_model_path() -> Option<PathBuf> {
    models_dir().map(|d| d.join(OBJECT_MODEL_NAME))
}

pub fn pose_model_ready() -> bool {
    pose_model_path().map(|p| p.is_file()).unwrap_or(false)
}

pub fn object_model_ready() -> bool {
    object_model_path().map(|p| p.is_file()).unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Model download (async)
// ---------------------------------------------------------------------------

async fn download_model(
    url: &str,
    dest: &Path,
    set_active: impl Fn(&mut SaliencyDownloadProgress, bool),
    set_bytes: impl Fn(&mut SaliencyDownloadProgress, u64, Option<u64>),
    prog: &Mutex<SaliencyDownloadProgress>,
) -> anyhow::Result<()> {
    if dest.is_file() {
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()?;

    let head = client.head(url).send().await.ok();
    let content_length = head.and_then(|r| r.content_length());

    {
        let mut p = prog.lock().unwrap();
        set_active(&mut p, true);
        set_bytes(&mut p, 0, content_length);
        p.error = None;
    }

    let resp = client.get(url).send().await?.error_for_status()?;
    let mut buf: Vec<u8> = Vec::new();
    if let Some(cl) = content_length {
        buf.reserve(cl as usize);
    }

    let start = std::time::Instant::now();
    let mut bytes_done: u64 = 0;
    let mut resp = resp;
    while let Some(chunk) = resp.chunk().await? {
        bytes_done += chunk.len() as u64;
        buf.extend_from_slice(&chunk);
        // Update progress roughly every 64 KB to avoid lock contention.
        if bytes_done % (64 * 1024) < chunk.len() as u64 {
            let _ = start.elapsed(); // keep borrow alive
            let mut p = prog.lock().unwrap();
            set_bytes(&mut p, bytes_done, content_length);
        }
    }

    // Write to a temp file first, then rename atomically.
    let tmp = dest.with_extension("tmp");
    std::fs::write(&tmp, &buf).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, dest).with_context(|| format!("renaming to {}", dest.display()))?;

    {
        let mut p = prog.lock().unwrap();
        set_active(&mut p, false);
    }

    Ok(())
}

/// Ensure the pose model (yolov8n-pose.onnx) is present.
pub async fn ensure_pose_model(state: Arc<AppState>) -> anyhow::Result<()> {
    let dest = pose_model_path().ok_or_else(|| anyhow::anyhow!("models directory unavailable"))?;
    download_model(
        POSE_MODEL_URL,
        &dest,
        |p, v| p.pose_active = v,
        |p, done, total| {
            p.pose_bytes_done = done;
            p.pose_bytes_total = total;
        },
        &state.saliency_download,
    )
    .await
    .inspect_err(|e| {
        if let Ok(mut p) = state.saliency_download.lock() {
            p.pose_active = false;
            p.error = Some(e.to_string());
        }
    })
}

/// Ensure the object model (yolov8n.onnx) is present.
pub async fn ensure_object_model(state: Arc<AppState>) -> anyhow::Result<()> {
    let dest =
        object_model_path().ok_or_else(|| anyhow::anyhow!("models directory unavailable"))?;
    download_model(
        OBJECT_MODEL_URL,
        &dest,
        |p, v| p.object_active = v,
        |p, done, total| {
            p.object_bytes_done = done;
            p.object_bytes_total = total;
        },
        &state.saliency_download,
    )
    .await
    .inspect_err(|e| {
        if let Ok(mut p) = state.saliency_download.lock() {
            p.object_active = false;
            p.error = Some(e.to_string());
        }
    })
}

// ---------------------------------------------------------------------------
// Loaded model cache
// ---------------------------------------------------------------------------

type OnnxModel = RunnableModel<TypedFact, Box<dyn TypedOp>, Graph<TypedFact, Box<dyn TypedOp>>>;

struct SaliencyModels {
    pose: OnnxModel,
    object: Option<OnnxModel>,
}

static MODEL_CACHE: Mutex<Option<Arc<SaliencyModels>>> = Mutex::new(None);

fn load_models(use_object: bool) -> anyhow::Result<Arc<SaliencyModels>> {
    // Return cached instance if compatible.
    if let Some(m) = MODEL_CACHE.lock().unwrap().clone()
        && (!use_object || m.object.is_some())
    {
        return Ok(m);
    }

    let pose_path =
        pose_model_path().ok_or_else(|| anyhow::anyhow!("models directory unavailable"))?;

    let pose = tract_onnx::onnx()
        .model_for_path(&pose_path)
        .context("loading yolov8n-pose.onnx")?
        .with_input_fact(
            0,
            InferenceFact::dt_shape(f32::datum_type(), tvec![1, 3, 640, 640]),
        )?
        .into_optimized()
        .context("optimising yolov8n-pose.onnx")?
        .into_runnable()
        .context("making yolov8n-pose.onnx runnable")?;

    let object = if use_object {
        let obj_path =
            object_model_path().ok_or_else(|| anyhow::anyhow!("models directory unavailable"))?;
        Some(
            tract_onnx::onnx()
                .model_for_path(&obj_path)
                .context("loading yolov8n.onnx")?
                .with_input_fact(
                    0,
                    InferenceFact::dt_shape(f32::datum_type(), tvec![1, 3, 640, 640]),
                )?
                .into_optimized()
                .context("optimising yolov8n.onnx")?
                .into_runnable()
                .context("making yolov8n.onnx runnable")?,
        )
    } else {
        None
    };

    let models = Arc::new(SaliencyModels { pose, object });
    *MODEL_CACHE.lock().unwrap() = Some(models.clone());
    Ok(models)
}

// ---------------------------------------------------------------------------
// Input preprocessing (shared for both models)
// ---------------------------------------------------------------------------

/// Resize `img` to fit within YOLO_SIZE × YOLO_SIZE while preserving aspect
/// ratio, pad with black to reach exactly YOLO_SIZE × YOLO_SIZE, and return
/// the scale factor used (multiply output coords by 1/scale to get
/// normalised [0,1] coords relative to the *original* image).
fn prep_yolo_input(img: &DynamicImage) -> (image::RgbImage, f32) {
    let orig_w = img.width();
    let orig_h = img.height();
    let scale = (YOLO_SIZE as f32 / orig_w as f32).min(YOLO_SIZE as f32 / orig_h as f32);
    let new_w = (orig_w as f32 * scale).round() as u32;
    let new_h = (orig_h as f32 * scale).round() as u32;

    let resized = img
        .resize_exact(new_w, new_h, image::imageops::FilterType::Triangle)
        .to_rgb8();

    let mut padded = image::RgbImage::from_pixel(YOLO_SIZE, YOLO_SIZE, image::Rgb([0u8, 0u8, 0u8]));
    for y in 0..new_h {
        for x in 0..new_w {
            padded.put_pixel(x, y, *resized.get_pixel(x, y));
        }
    }
    (padded, scale)
}

/// Build [1, 3, H, W] float32 tensor from an RGB image.
/// YOLOv8 normalises to [0, 1] (divide by 255).
fn rgb_to_nchw_tensor(rgb: &image::RgbImage) -> anyhow::Result<Tensor> {
    let h = rgb.height() as usize;
    let w = rgb.width() as usize;
    let mut data = vec![0_f32; 3 * h * w];
    for (idx, pixel) in rgb.pixels().enumerate() {
        data[idx] = pixel[0] as f32 / 255.0;
        data[h * w + idx] = pixel[1] as f32 / 255.0;
        data[2 * h * w + idx] = pixel[2] as f32 / 255.0;
    }
    Ok(tract_ndarray::Array4::from_shape_vec((1, 3, h, w), data)?.into())
}

// ---------------------------------------------------------------------------
// Pose model inference → SalientPoint
// ---------------------------------------------------------------------------
//
// YOLOv8n-pose ONNX output: [1, 56, 8400]
//   channels 0–3 : cx, cy, w, h  (absolute pixels in 640-px space)
//   channel  4   : person confidence (0–1, sigmoid applied in graph)
//   channels 5+  : 17 keypoints × 3  (x_640, y_640, visibility)
//     keypoint 0 = nose,  1 = left eye,  2 = right eye,
//     3 = left ear, 4 = right ear
//
// We return the nose keypoint when visible, otherwise upper bbox centre.
// Coordinates are converted back to original-image-normalised [0, 1].

fn run_pose(model: &OnnxModel, img: &DynamicImage) -> Option<SalientPoint> {
    let orig_w = img.width() as f32;
    let orig_h = img.height() as f32;
    let (padded, scale) = prep_yolo_input(img);
    let tensor = rgb_to_nchw_tensor(&padded).ok()?;
    let outputs = model.run(tvec![tensor.into()]).ok()?;
    let flat = outputs[0].as_slice::<f32>().ok()?;

    // flat layout: [1, 56, 8400] → flat[c * 8400 + i]
    let n = flat.len() / 56;
    if n == 0 {
        return None;
    }

    // Find the most confident person detection.
    let (best_i, best_conf) = (0..n)
        .map(|i| (i, flat[4 * n + i]))
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))?;

    if best_conf < CONF_THRESHOLD {
        return None;
    }

    let cx_640 = flat[best_i];
    let cy_640 = flat[n + best_i];
    let h_640 = flat[3 * n + best_i];

    // Try nose keypoint (kp 0): channels 5, 6, 7.
    let kp_x = flat[5 * n + best_i];
    let kp_y = flat[6 * n + best_i];
    let kp_v = flat[7 * n + best_i];

    let (focus_x_640, focus_y_640) = if kp_v >= KP_VIS_THRESHOLD {
        (kp_x, kp_y)
    } else {
        // Fallback: top 30% of the bounding box.
        (cx_640, cy_640 - h_640 * 0.30)
    };

    // Convert from padded-640-space back to original image normalised coords.
    Some(SalientPoint {
        cx: (focus_x_640 / scale).clamp(0.0, orig_w) / orig_w,
        cy: (focus_y_640 / scale).clamp(0.0, orig_h) / orig_h,
    })
}

// ---------------------------------------------------------------------------
// Object model inference → SalientPoint
// ---------------------------------------------------------------------------
//
// YOLOv8n ONNX output: [1, 84, 8400]
//   channels 0–3  : cx, cy, w, h  (absolute pixels in 640-px space)
//   channels 4–83 : 80 class scores (0–1, no separate objectness)
//
// "Confidence" = max class score.  We return the centre of the highest-
// confidence detection.

fn run_objects(model: &OnnxModel, img: &DynamicImage) -> Option<SalientPoint> {
    let orig_w = img.width() as f32;
    let orig_h = img.height() as f32;
    let (padded, scale) = prep_yolo_input(img);
    let tensor = rgb_to_nchw_tensor(&padded).ok()?;
    let outputs = model.run(tvec![tensor.into()]).ok()?;
    let flat = outputs[0].as_slice::<f32>().ok()?;

    let n = flat.len() / 84;
    if n == 0 {
        return None;
    }

    let (best_i, best_conf) = (0..n)
        .map(|i| {
            let conf = (4..84).map(|c| flat[c * n + i]).fold(0_f32, f32::max);
            (i, conf)
        })
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))?;

    if best_conf < CONF_THRESHOLD {
        return None;
    }

    let cx_640 = flat[best_i];
    let cy_640 = flat[n + best_i];

    Some(SalientPoint {
        cx: (cx_640 / scale).clamp(0.0, orig_w) / orig_w,
        cy: (cy_640 / scale).clamp(0.0, orig_h) / orig_h,
    })
}

// ---------------------------------------------------------------------------
// Public API: detect salient point (blocking — intended for spawn_blocking)
// ---------------------------------------------------------------------------

/// Detect the most salient focus point in `img`.
///
/// 1. Run YOLOv8n-pose.  If a person with a reliable confidence is found,
///    return the nose keypoint (or upper bbox centre as fallback).
/// 2. If no person found AND `use_objects` is true AND the object model is
///    ready, run YOLOv8n and return the centre of the most confident object.
/// 3. Return `None` if nothing is detected above the threshold (callers
///    fall back to orientation-based gravity).
///
/// This function is **blocking** and should be called inside `spawn_blocking`.
pub fn detect_salient_point(img: &DynamicImage, use_objects: bool) -> Option<SalientPoint> {
    if !pose_model_ready() {
        return None;
    }

    // Load (and cache) the models.
    let effective_use_objects = use_objects && object_model_ready();
    let models = match load_models(effective_use_objects) {
        Ok(m) => m,
        Err(_) => return None,
    };

    // Try pose first.
    if let Some(sp) = run_pose(&models.pose, img) {
        return Some(sp);
    }

    // Fallback: object detection.
    if effective_use_objects && let Some(obj_model) = &models.object {
        return run_objects(obj_model, img);
    }

    None
}

/// Detect salient points for a batch of images.  Returns one `Option<SalientPoint>`
/// per path, in the same order.  Reads and decodes each image file from disk.
pub fn detect_salient_points_for_files(
    paths: &[PathBuf],
    use_objects: bool,
) -> Vec<Option<SalientPoint>> {
    paths
        .iter()
        .map(|p| {
            std::fs::read(p)
                .ok()
                .and_then(|d| image::load_from_memory(&d).ok())
                .and_then(|img| detect_salient_point(&img, use_objects))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// API handlers
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct SaliencyStatusResponse {
    pub pose_ready: bool,
    pub object_ready: bool,
    pub download: SaliencyDownloadProgress,
}

pub async fn api_saliency_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<SaliencyStatusResponse>, AppError> {
    let download = state.saliency_download.lock().unwrap().clone();
    Ok(Json(SaliencyStatusResponse {
        pose_ready: pose_model_ready(),
        object_ready: object_model_ready(),
        download,
    }))
}

pub async fn api_saliency_ensure_pose(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // Skip if already running or model already present.
    {
        let p = state.saliency_download.lock().unwrap();
        if p.pose_active {
            return (axum::http::StatusCode::ACCEPTED, "already downloading").into_response();
        }
    }
    if pose_model_ready() {
        return Json(serde_json::json!({"status": "already_present"})).into_response();
    }
    tokio::spawn(async move {
        let _ = ensure_pose_model(state).await;
    });
    Json(serde_json::json!({"status": "started"})).into_response()
}

pub async fn api_saliency_ensure_object(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    {
        let p = state.saliency_download.lock().unwrap();
        if p.object_active {
            return (axum::http::StatusCode::ACCEPTED, "already downloading").into_response();
        }
    }
    if object_model_ready() {
        return Json(serde_json::json!({"status": "already_present"})).into_response();
    }
    tokio::spawn(async move {
        let _ = ensure_object_model(state).await;
    });
    Json(serde_json::json!({"status": "started"})).into_response()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn salient_point_fields() {
        let sp = SalientPoint { cx: 0.5, cy: 0.3 };
        assert!((sp.cx - 0.5).abs() < 1e-6);
        assert!((sp.cy - 0.3).abs() < 1e-6);
    }

    #[test]
    fn model_paths_under_data_dir() {
        if let Some(p) = pose_model_path() {
            assert!(p.ends_with(POSE_MODEL_NAME));
        }
        if let Some(p) = object_model_path() {
            assert!(p.ends_with(OBJECT_MODEL_NAME));
        }
    }
}
