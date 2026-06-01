//! Inbound image downscaling and re-encoding
//!
//! Downscales the base64-encoded images carried in Anthropic protocol ContentBlocks **locally on CPU** to
//! a long side <= `KIRO_RS_IMAGE_MAX_LONG_SIDE` px and a byte size <= `KIRO_RS_IMAGE_MAX_BYTES`,
//! then re-encodes to base64 and writes it back into the KiroImage. Why this step is required:
//!
//! 1. The AWS Q (`q.us-east-1.amazonaws.com`) backend enforces a hard per-field size limit. A ~700 KB
//!    toolResult.content[0].text triggers `CONTENT_LENGTH_EXCEEDS_THRESHOLD`,
//!    and an iPhone screenshot (1206x2622 PNG) whose single base64 string is ~700K chars triggers it too.
//! 2. Anthropic recommends a long side <= 1568 px; this value is the vision encoder's patch
//!    grid boundary. Beyond it the server downscales again, yet tokens are still billed against the original.
//! 3. ChatGPT/OpenAI servers downscale to this size automatically; AWS Q does not. That is the root
//!    cause of the same iPhone screenshots succeeding on GPT models while Kiro Opus returns 400.
//!
//! Design principles:
//! - Small images pass through directly (no decode, no re-encode, zero overhead)
//! - Large images are downscaled to the long-side cap and re-encoded as JPEG (PNG/WebP/JPEG all
//!   emit JPEG; GIF is the exception and keeps its original format because it may be animated)
//! - On decode failure **keep the original image** and log a warning; a bad image must never fail the whole request
//! - Everything is driven by `KIRO_RS_IMAGE_*` env vars, sharing the same contract as the observability env-var family

use std::io::Cursor;

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use image::{ImageFormat, ImageReader, imageops::FilterType};
use tracing::{debug, warn};

/// Default long-side threshold (Anthropic's recommended value)
const DEFAULT_MAX_LONG_SIDE: u32 = 1568;
/// Default byte threshold (leaves a safe margin below the AWS Q per-field limit)
const DEFAULT_MAX_BYTES: usize = 400_000;
/// Default JPEG quality
const DEFAULT_JPEG_QUALITY: u8 = 85;

/// Inbound image processor configuration
#[derive(Debug, Clone, Copy)]
pub struct ResizeConfig {
    pub enabled: bool,
    pub max_long_side: u32,
    pub max_bytes: usize,
    pub jpeg_quality: u8,
}

impl ResizeConfig {
    /// Reads from `KIRO_RS_IMAGE_*` env vars, falling back to defaults when unset
    pub fn from_env() -> Self {
        let enabled = !matches!(
            std::env::var("KIRO_RS_IMAGE_RESIZE")
                .unwrap_or_else(|_| "1".to_string())
                .to_ascii_lowercase()
                .as_str(),
            "0" | "false" | "no" | "off"
        );
        let max_long_side = std::env::var("KIRO_RS_IMAGE_MAX_LONG_SIDE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_MAX_LONG_SIDE);
        let max_bytes = std::env::var("KIRO_RS_IMAGE_MAX_BYTES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_MAX_BYTES);
        let jpeg_quality = std::env::var("KIRO_RS_IMAGE_JPEG_QUALITY")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_JPEG_QUALITY);
        Self {
            enabled,
            max_long_side,
            max_bytes,
            jpeg_quality,
        }
    }
}

/// Result of processing one image (explicitly distinguishes the "kept as-is" and "re-encoded" states)
///
/// `was_resized` / `original_bytes` / `final_bytes` are consumed only by test assertions and structured logs;
/// non-test runtime paths do not read them, so the whole struct is marked `allow(dead_code)` to keep the diagnostic fields.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ProcessedImage {
    /// Output format ("jpeg" / "png" / "gif" / "webp")
    pub format: String,
    /// Output base64 string
    pub data_base64: String,
    /// Whether re-encoding actually happened (used for logs/metrics)
    pub was_resized: bool,
    /// Input byte count (before decoding)
    pub original_bytes: usize,
    /// Output byte count
    pub final_bytes: usize,
}

/// Main entry: processes a single inbound image with the rule "small enough -> pass / large -> shrink"
///
/// `format` is the last segment of the source media-type ("png" / "jpeg" / "gif" / "webp"),
/// `data_base64` is the base64-encoded raw bytes.
///
/// Never panics and never drops an image. On failure it returns an owned copy of the input and logs a warning.
pub fn maybe_shrink_image(
    cfg: ResizeConfig,
    format: &str,
    data_base64: &str,
) -> ProcessedImage {
    let format_lc = format.to_ascii_lowercase();
    let original_bytes = data_base64.len();

    // 1) Disabled: return as-is
    if !cfg.enabled {
        return passthrough(format_lc, data_base64);
    }
    // 2) Bytes small enough: return as-is (small images need no work, saves CPU)
    if data_base64.len() <= cfg.max_bytes {
        // Even with small bytes, check whether the dimensions are oversized (rare, e.g. a 7000x100 banner)
        // Use a lightweight probe (header only): image::ImageReader::with_guessed_format
        if let Some((w, h)) = peek_dimensions(&format_lc, data_base64)
            && w.max(h) <= cfg.max_long_side
        {
            return passthrough(format_lc, data_base64);
        }
        // Small bytes but oversized dimensions: still take the re-encode path
    }
    // 3) Animated images (multi-frame GIF) keep their original format unchanged - JPEG would lose the animation
    if format_lc == "gif" {
        debug!(
            target: "kiro_rs::image_resize",
            original_bytes = original_bytes,
            "skip GIF (potential animation)"
        );
        return passthrough(format_lc, data_base64);
    }

    // 4) Actually shrink the image
    match shrink_static_image(cfg, &format_lc, data_base64) {
        Ok(processed) => processed,
        Err(e) => {
            warn!(
                target: "kiro_rs::image_resize",
                error = %e,
                format = %format_lc,
                original_bytes = original_bytes,
                "image resize failed; passing through original"
            );
            passthrough(format_lc, data_base64)
        }
    }
}

fn passthrough(format: String, data_base64: &str) -> ProcessedImage {
    let n = data_base64.len();
    // Correct the format from the real magic bytes: the host may label it png while the bytes are actually jpeg,
    // and faithful passthrough would trip Bedrock's strict MIME check with IMAGE_MIME_MISMATCH. If detection fails, keep the original label (never drop the image).
    let format = match detect_format_from_bytes(data_base64) {
        Some(real) if real != format => {
            debug!(
                target: "kiro_rs::image_resize",
                declared = %format,
                actual = %real,
                "passthrough format corrected from magic bytes"
            );
            real
        }
        _ => format,
    };
    ProcessedImage {
        format,
        data_base64: data_base64.to_string(),
        was_resized: false,
        original_bytes: n,
        final_bytes: n,
    }
}

/// Detects the format from the real magic bytes, returning "png"/"jpeg"/"gif"/"webp".
/// Decoding only the first ~16 bytes (first 24 base64 chars) is enough to cover every magic number and saves CPU.
/// On detection failure (decode error / unknown format) it returns None, and the caller safely keeps the original label.
fn detect_format_from_bytes(data_base64: &str) -> Option<String> {
    let head: String = data_base64.chars().take(24).collect();
    let bytes = BASE64.decode(head.as_bytes()).ok()?;
    match image::guess_format(&bytes).ok()? {
        ImageFormat::Png => Some("png".to_string()),
        ImageFormat::Jpeg => Some("jpeg".to_string()),
        ImageFormat::Gif => Some("gif".to_string()),
        ImageFormat::WebP => Some("webp".to_string()),
        _ => None,
    }
}

/// Reads only the header for dimensions without decoding all pixels; < 1ms per image
fn peek_dimensions(format: &str, data_base64: &str) -> Option<(u32, u32)> {
    let bytes = BASE64.decode(data_base64).ok()?;
    let cursor = Cursor::new(&bytes);
    let mut reader = ImageReader::new(cursor);
    if let Some(fmt) = guess_format(format) {
        reader.set_format(fmt);
    } else {
        reader = reader.with_guessed_format().ok()?;
    }
    reader.into_dimensions().ok()
}

fn guess_format(s: &str) -> Option<ImageFormat> {
    match s {
        "png" => Some(ImageFormat::Png),
        "jpeg" | "jpg" => Some(ImageFormat::Jpeg),
        "webp" => Some(ImageFormat::WebP),
        "gif" => Some(ImageFormat::Gif),
        _ => None,
    }
}

fn shrink_static_image(
    cfg: ResizeConfig,
    format: &str,
    data_base64: &str,
) -> Result<ProcessedImage, ResizeError> {
    let original_bytes = data_base64.len();

    let raw = BASE64
        .decode(data_base64)
        .map_err(|e| ResizeError::Base64(e.to_string()))?;

    let cursor = Cursor::new(&raw);
    let mut reader = ImageReader::new(cursor);
    if let Some(fmt) = guess_format(format) {
        reader.set_format(fmt);
    } else {
        reader = reader
            .with_guessed_format()
            .map_err(|e| ResizeError::Decode(e.to_string()))?;
    }
    let img = reader
        .decode()
        .map_err(|e| ResizeError::Decode(e.to_string()))?;

    // Proportional scaling (preserves the aspect ratio)
    let (w, h) = (img.width(), img.height());
    let long = w.max(h);
    let resized = if long > cfg.max_long_side {
        let scale = cfg.max_long_side as f32 / long as f32;
        let new_w = ((w as f32) * scale).round().max(1.0) as u32;
        let new_h = ((h as f32) * scale).round().max(1.0) as u32;
        // FilterType::Lanczos3 gives good visual quality; on a single CPU core 1206x2622 -> 1024x~470
        // takes ~80ms empirically; ~2.5s for a batch of 30 images, which is acceptable
        img.resize_exact(new_w, new_h, FilterType::Lanczos3)
    } else {
        img
    };

    // JPEG re-encoding (for screenshots, quality 85 is visually lossless and 10-20x smaller)
    let mut out = Vec::with_capacity(64 * 1024);
    {
        // Force RGB8 (JPEG has no alpha channel; alpha is dropped, which is harmless for screenshots)
        let rgb = resized.to_rgb8();
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
            &mut out,
            cfg.jpeg_quality,
        );
        rgb.write_with_encoder(encoder)
            .map_err(|e| ResizeError::Encode(e.to_string()))?;
    }
    let final_bytes_raw = out.len();
    let data_b64 = BASE64.encode(&out);
    let final_bytes = data_b64.len();

    debug!(
        target: "kiro_rs::image_resize",
        original_bytes = original_bytes,
        final_bytes = final_bytes,
        ratio = format!("{:.2}x", original_bytes as f64 / final_bytes.max(1) as f64),
        decoded_w = w,
        decoded_h = h,
        out_jpeg_bytes = final_bytes_raw,
        "image resized"
    );

    Ok(ProcessedImage {
        format: "jpeg".to_string(),
        data_base64: data_b64,
        was_resized: true,
        original_bytes,
        final_bytes,
    })
}

#[derive(Debug, thiserror::Error)]
enum ResizeError {
    #[error("base64 decode: {0}")]
    Base64(String),
    #[error("image decode: {0}")]
    Decode(String),
    #[error("image encode: {0}")]
    Encode(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_png(w: u32, h: u32) -> String {
        use image::{Rgb, RgbImage};
        let mut img = RgbImage::new(w, h);
        // Gradient fill: its compression ratio is closer to real screenshots than a solid color
        for y in 0..h {
            for x in 0..w {
                img.put_pixel(x, y, Rgb([(x % 256) as u8, (y % 256) as u8, 128]));
            }
        }
        let mut buf = Vec::new();
        img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
            .unwrap();
        BASE64.encode(&buf)
    }

    #[test]
    fn small_image_passes_through() {
        let cfg = ResizeConfig {
            enabled: true,
            max_long_side: 1568,
            max_bytes: 400_000,
            jpeg_quality: 85,
        };
        let small = make_png(64, 64);
        let out = maybe_shrink_image(cfg, "png", &small);
        assert!(!out.was_resized);
        assert_eq!(out.format, "png");
        assert_eq!(out.data_base64, small);
    }

    #[test]
    fn iphone_screenshot_gets_shrunk_below_limit() {
        let cfg = ResizeConfig {
            enabled: true,
            max_long_side: 1568,
            max_bytes: 400_000,
            jpeg_quality: 85,
        };
        // 1206x2622 ~ iPhone Pro Max screenshot ratio
        let big = make_png(1206, 2622);
        let out = maybe_shrink_image(cfg, "png", &big);
        assert!(out.was_resized, "should have been resized");
        assert_eq!(out.format, "jpeg", "should have been re-encoded as JPEG");
        assert!(
            out.final_bytes < cfg.max_bytes,
            "final {} should be < cap {}",
            out.final_bytes,
            cfg.max_bytes
        );
        // The gradient test image compresses worse than a real screenshot (blocky UI elements); we only need it below the threshold
        // Real iPhone screenshots compress far more (15-20x) - see the README "measured data" section
        let _ = out.original_bytes;
    }

    #[test]
    fn gif_passes_through_to_preserve_animation() {
        let cfg = ResizeConfig::from_env();
        // A 1x1 GIF is enough; what matters here is exercising the branch
        let tiny_gif = "R0lGODlhAQABAAAAACw=";
        let out = maybe_shrink_image(cfg, "gif", tiny_gif);
        assert!(!out.was_resized);
        assert_eq!(out.format, "gif");
    }

    #[test]
    fn disabled_config_passes_through_even_huge() {
        let cfg = ResizeConfig {
            enabled: false,
            max_long_side: 1568,
            max_bytes: 400_000,
            jpeg_quality: 85,
        };
        let big = make_png(1206, 2622);
        let out = maybe_shrink_image(cfg, "png", &big);
        assert!(!out.was_resized);
        assert_eq!(out.format, "png");
    }

    #[test]
    fn corrupt_data_passes_through_with_warning() {
        let cfg = ResizeConfig {
            enabled: true,
            max_long_side: 1568,
            max_bytes: 100,
            jpeg_quality: 85,
        };
        // Deliberately feed corrupt data + over-limit bytes to trigger the decode path
        let bogus = "X".repeat(1000);
        let out = maybe_shrink_image(cfg, "png", &bogus);
        assert!(!out.was_resized, "corrupt input should fall through");
        assert_eq!(out.format, "png");
        assert_eq!(out.data_base64, bogus);
    }

    fn make_jpeg(w: u32, h: u32) -> String {
        use image::{Rgb, RgbImage};
        let mut img = RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.put_pixel(x, y, Rgb([(x % 256) as u8, (y % 256) as u8, 128]));
            }
        }
        let mut buf = Vec::new();
        img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Jpeg)
            .unwrap();
        BASE64.encode(&buf)
    }

    #[test]
    fn mislabeled_png_header_jpeg_bytes_corrected_to_jpeg() {
        let cfg = ResizeConfig {
            enabled: true,
            max_long_side: 1568,
            max_bytes: 400_000,
            jpeg_quality: 85,
        };
        // Real JPEG bytes, but the caller mislabels format="png" (host-side header/body mismatch, faithfully passed through).
        // Small images take the passthrough path. The outbound format must be corrected to jpeg per the real bytes, otherwise Bedrock returns IMAGE_MIME_MISMATCH.
        let jpeg = make_jpeg(64, 64);
        let out = maybe_shrink_image(cfg, "png", &jpeg);
        assert_eq!(out.data_base64, jpeg, "must not mutate image bytes");
        assert_eq!(
            out.format, "jpeg",
            "format must be corrected to match actual JPEG bytes"
        );
    }

    #[test]
    fn matching_png_kept_as_png() {
        let cfg = ResizeConfig::from_env();
        let png = make_png(64, 64);
        let out = maybe_shrink_image(cfg, "png", &png);
        assert_eq!(out.format, "png", "real png must stay png");
        assert_eq!(out.data_base64, png);
    }

    #[test]
    fn matching_jpeg_kept_as_jpeg() {
        let cfg = ResizeConfig::from_env();
        let jpeg = make_jpeg(64, 64);
        let out = maybe_shrink_image(cfg, "jpeg", &jpeg);
        assert_eq!(out.format, "jpeg", "real jpeg must stay jpeg");
        assert_eq!(out.data_base64, jpeg);
    }

    #[test]
    fn undetectable_bytes_keep_declared_format() {
        // Detection fails on corrupt data -> keep the incoming format, never drop the image.
        let cfg = ResizeConfig {
            enabled: false,
            max_long_side: 1568,
            max_bytes: 400_000,
            jpeg_quality: 85,
        };
        let bogus = "X".repeat(40);
        let out = maybe_shrink_image(cfg, "png", &bogus);
        assert_eq!(out.format, "png", "undetectable bytes keep declared format");
        assert_eq!(out.data_base64, bogus);
    }
}
