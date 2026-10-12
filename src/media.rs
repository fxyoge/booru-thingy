use crate::App;
use md5::Md5;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};

pub struct Metadata {
    pub sha256: String,
    pub md5: String,
    pub ext: &'static str,
    pub content_type: &'static str,
    pub width: i32,
    pub height: i32,
    pub duration: Option<f64>,
    pub size: i64,
}
pub struct Variant {
    pub kind: String,
    pub key: String,
    pub ext: String,
    pub content_type: String,
    pub width: i32,
    pub height: i32,
    pub size: i64,
}
async fn command(program: &str, args: &[&str]) -> anyhow::Result<Vec<u8>> {
    let output = Command::new(program)
        .args(args)
        .kill_on_drop(true)
        .output()
        .await?;
    anyhow::ensure!(output.status.success(), "media tool failed: {program}");
    Ok(output.stdout)
}
pub async fn inspect(path: &Path) -> anyhow::Result<Metadata> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut sha = Sha256::new();
    let mut md5 = Md5::new();
    let mut size = 0_i64;
    let mut header = Vec::new();
    let mut bytes = vec![0_u8; 65536];
    loop {
        let n = file.read(&mut bytes).await?;
        if n == 0 {
            break;
        }
        if header.is_empty() {
            header.extend_from_slice(&bytes[..n.min(64)]);
        }
        sha.update(&bytes[..n]);
        md5.update(&bytes[..n]);
        size += n as i64;
    }
    let (ext, content_type) =
        format(&header).ok_or_else(|| anyhow::anyhow!("unsupported media format"))?;
    let (width, height, duration) = if matches!(ext, "jpg" | "png" | "gif" | "webp") {
        let output = command("vipsheader", &["-a", path.to_str().unwrap()]).await?;
        let text = String::from_utf8_lossy(&output);
        let value = |name: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(&format!("{name}: ")))
        };
        let number = |name: &str| {
            value(name)
                .and_then(|v| v.trim().parse::<i64>().ok())
                .unwrap_or(0)
        };
        let mut width = number("width");
        let mut height = if number("page-height") > 0 {
            number("page-height")
        } else {
            number("height")
        };
        if (5..=8).contains(&number("orientation")) {
            std::mem::swap(&mut width, &mut height);
        }
        let duration = if number("n-pages") > 1 {
            value("delay")
                .map(|v| {
                    v.split_whitespace()
                        .filter_map(|v| v.parse::<f64>().ok())
                        .sum::<f64>()
                        / 1000.0
                })
                .filter(|v| *v > 0.0)
        } else {
            None
        };
        (width, height, duration)
    } else {
        let output = command(
            "ffprobe",
            &[
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=width,height,duration:stream_side_data=rotation:format=duration",
                "-of",
                "json",
                path.to_str().unwrap(),
            ],
        )
        .await?;
        let probe: Value = serde_json::from_slice(&output)?;
        let stream = probe["streams"]
            .as_array()
            .and_then(|s| s.first())
            .ok_or_else(|| anyhow::anyhow!("media contains no visual stream"))?;
        let mut width = stream["width"].as_i64().unwrap_or(0);
        let mut height = stream["height"].as_i64().unwrap_or(0);
        if stream["side_data_list"].as_array().is_some_and(|list| {
            list.iter().any(|entry| {
                entry["rotation"]
                    .as_i64()
                    .is_some_and(|r| r.abs() % 180 == 90)
            })
        }) {
            std::mem::swap(&mut width, &mut height);
        }
        let duration = stream["duration"]
            .as_str()
            .or_else(|| probe["format"]["duration"].as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v > 0.0);
        (width, height, duration)
    };
    anyhow::ensure!(
        width > 0
            && height > 0
            && width <= 32768
            && height <= 32768
            && width * height <= 100_000_000,
        "invalid or excessive image dimensions"
    );
    Ok(Metadata {
        sha256: hex::encode(sha.finalize()),
        md5: hex::encode(md5.finalize()),
        ext,
        content_type,
        width: width as i32,
        height: height as i32,
        duration,
        size,
    })
}
fn format(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(("jpg", "image/jpeg"))
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(("png", "image/png"))
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(("gif", "image/gif"))
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some(("webp", "image/webp"))
    } else if bytes.get(4..8) == Some(b"ftyp")
        && bytes.get(8..12).is_some_and(|b| {
            b != b"avif" && b != b"avis" && b != b"heic" && b != b"heix" && b != b"mif1"
        })
    {
        Some(("mp4", "video/mp4"))
    } else if bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) && bytes.windows(4).any(|b| b == b"webm")
    {
        Some(("webm", "video/webm"))
    } else {
        None
    }
}
pub async fn variants(
    app: &App,
    metadata: &Metadata,
    input: &Path,
    directory: &Path,
) -> anyhow::Result<Vec<Variant>> {
    let original_key = format!("media/{}/original.{}", metadata.sha256, metadata.ext);
    app.storage
        .put(&original_key, input, metadata.content_type)
        .await?;
    let mut variants = vec![];
    for kind in ["original", "full"] {
        variants.push(Variant {
            kind: kind.into(),
            key: original_key.clone(),
            ext: metadata.ext.into(),
            content_type: metadata.content_type.into(),
            width: metadata.width,
            height: metadata.height,
            size: metadata.size,
        });
    }
    let frame: PathBuf = directory.join("frame.png");
    // libvips loads the first image frame; ffmpeg extracts video poster frames.
    if matches!(metadata.ext, "mp4" | "webm") {
        command(
            "ffmpeg",
            &[
                "-v",
                "error",
                "-nostdin",
                "-y",
                "-threads",
                "1",
                "-i",
                input.to_str().unwrap(),
                "-frames:v",
                "1",
                frame.to_str().unwrap(),
            ],
        )
        .await?;
    }
    let source = if matches!(metadata.ext, "mp4" | "webm") {
        &frame
    } else {
        input
    };
    for (kind, max) in [
        ("180x180", 180),
        ("360x360", 360),
        ("720x720", 720),
        ("sample", 1600),
    ] {
        if kind == "sample"
            && (metadata.duration.is_some()
                || metadata.ext == "gif"
                || matches!(metadata.ext, "mp4" | "webm"))
        {
            variants.push(Variant {
                kind: kind.into(),
                key: original_key.clone(),
                ext: metadata.ext.into(),
                content_type: metadata.content_type.into(),
                width: metadata.width,
                height: metadata.height,
                size: metadata.size,
            });
            continue;
        }
        let output = directory.join(format!("{kind}.jpg"));
        command(
            "vipsthumbnail",
            &[
                source.to_str().unwrap(),
                "--size",
                &format!("{max}x{max}>"),
                "--output",
                &format!("{}[Q=85,strip,background=255 255 255]", output.display()),
            ],
        )
        .await?;
        let generated = command("vipsheader", &["-a", output.to_str().unwrap()]).await?;
        let text = String::from_utf8_lossy(&generated);
        let dimension = |name: &str| {
            text.lines().find_map(|l| {
                l.strip_prefix(&format!("{name}: "))
                    .and_then(|v| v.trim().parse::<i32>().ok())
            })
        };
        let width =
            dimension("width").ok_or_else(|| anyhow::anyhow!("could not read thumbnail width"))?;
        let height = dimension("height")
            .ok_or_else(|| anyhow::anyhow!("could not read thumbnail height"))?;
        let size = tokio::fs::metadata(&output).await?.len() as i64;
        let key = format!("media/{}/v1/{kind}.jpg", metadata.sha256);
        app.storage.put(&key, &output, "image/jpeg").await?;
        variants.push(Variant {
            kind: kind.into(),
            key,
            ext: "jpg".into(),
            content_type: "image/jpeg".into(),
            width,
            height,
            size,
        });
    }
    Ok(variants)
}
pub async fn with_timeout<T>(
    seconds: u64,
    future: impl std::future::Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    tokio::time::timeout(Duration::from_secs(seconds), future)
        .await
        .map_err(|_| anyhow::anyhow!("processing timed out"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn format_uses_bytes_not_filenames() {
        assert_eq!(format(b"GIF89a"), Some(("gif", "image/gif")));
        assert_eq!(format(b"<html>oops</html>"), None);
        assert_eq!(format(b"\0\0\0\x18ftypavif"), None);
    }
}
