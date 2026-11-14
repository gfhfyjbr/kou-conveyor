//! Pictures in prompts: what a paste, a drop or a file brings, made into what
//! every backend takes (PNG, JPEG, GIF or WebP, at most 2000 pixels a side and
//! 20 MB), with the thumbnail and the larger picture the field shows of it.
//! Everything here runs off the main thread; the field only shows the result.

use gpui::RenderImage;
use image::{
    DynamicImage, Frame, ImageDecoder as _, ImageFormat, ImageReader, imageops::FilterType,
    metadata::Orientation,
};
use std::{io::Cursor, path::Path, sync::Arc};

/// The most pictures one prompt brings: what kou's runner takes.
pub(crate) const MAX_PICTURES: usize = 20;
/// The most bytes a picture may have once it is made ready: what kou's runner takes.
pub(crate) const MAX_BYTES: usize = 20 << 20;
/// The longest side a picture keeps; a larger one is scaled down. Models
/// scale larger pictures down themselves and charge for the difference.
pub(crate) const MAX_SIDE: u32 = 2000;
/// The most bytes read of a picture before it is turned down.
pub(crate) const MAX_SOURCE: usize = 64 << 20;
/// A thumbnail's longer side, twice the strip's size for sharp displays.
const THUMB: u32 = 192;
/// The larger picture's longer side, for the preview over the transcript.
const LARGE: u32 = 1200;

/// A picture made ready for a prompt.
#[derive(Clone)]
pub(crate) struct Prepared {
    /// `image/png`, `image/jpeg`, `image/gif` or `image/webp`.
    pub mime: &'static str,
    /// The bytes a backend gets.
    pub data: Arc<[u8]>,
    /// Its size in pixels, as sent.
    pub width: u32,
    pub height: u32,
    /// What was done to it, for the user: `converted from TIFF`, `scaled down from 4032×3024`.
    pub note: Option<String>,
    /// Small, for the strip and the transcript.
    pub thumb: Arc<RenderImage>,
    /// Larger, for the preview.
    pub large: Arc<RenderImage>,
}

impl std::fmt::Debug for Prepared {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Prepared")
            .field("mime", &self.mime)
            .field("bytes", &self.data.len())
            .field("width", &self.width)
            .field("height", &self.height)
            .field("note", &self.note)
            .finish_non_exhaustive()
    }
}

impl Prepared {
    /// What the picture is, for a caption: `1920×1080 PNG · 240 KB`.
    pub(crate) fn describe(&self) -> String {
        describe(self.width, self.height, self.mime, self.data.len())
    }
}

/// What a picture is, for a caption: `1920×1080 PNG · 240 KB`.
pub(crate) fn describe(width: u32, height: u32, mime: &str, bytes: usize) -> String {
    let kind = mime.trim_start_matches("image/").to_uppercase();
    format!("{width}×{height} {kind} · {}", size(bytes))
}

/// A size in bytes, shortened.
pub(crate) fn size(count: usize) -> String {
    if count < 1024 {
        format!("{count} B")
    } else if count < 1024 * 1024 {
        format!("{:.0} KB", count as f64 / 1024.)
    } else {
        format!("{:.1} MB", count as f64 / (1024. * 1024.))
    }
}

/// How a prompt's text names its picture `n`, as the cockpits do.
pub(crate) fn label(number: usize) -> String {
    format!("[Image {number}]")
}

/// The numbers of the picture labels a text has, in order.
pub(crate) fn label_numbers(text: &str) -> Vec<usize> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("[Image ") {
        rest = &rest[at + "[Image ".len()..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if (1..=6).contains(&digits)
            && rest[digits..].starts_with(']')
            && let Ok(number) = rest[..digits].parse::<usize>()
            && number > 0
        {
            found.push(number);
        }
    }
    found
}

/// Whether a file is a picture this app reads, by its name.
pub(crate) fn is_picture_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff"
            )
        })
}

/// Makes a picture ready for a prompt: one a backend takes as it is stays as
/// it is; BMP, TIFF and anything larger than [`MAX_SIDE`] is converted and
/// scaled down. The answer is an error for the user when it is no picture
/// this app reads.
pub(crate) fn prepare(bytes: &[u8]) -> Result<Prepared, String> {
    if bytes.len() > MAX_SOURCE {
        return Err(format!(
            "it is {}; pictures of up to {} are read",
            size(bytes.len()),
            size(MAX_SOURCE)
        ));
    }
    let format = image::guess_format(bytes)
        .map_err(|_| "it is not a picture this app reads (PNG, JPEG, GIF, WebP, BMP, TIFF)")?;
    let kept = match format {
        ImageFormat::Png => Some("image/png"),
        ImageFormat::Jpeg => Some("image/jpeg"),
        ImageFormat::Gif => Some("image/gif"),
        ImageFormat::WebP => Some("image/webp"),
        ImageFormat::Bmp | ImageFormat::Tiff => None,
        other => {
            return Err(format!(
                "it is {}, which models do not take; PNG, JPEG, GIF, WebP, BMP and TIFF go",
                other.extensions_str().first().unwrap_or(&"of another kind")
            ));
        }
    };
    let mut decoder = ImageReader::with_format(Cursor::new(bytes), format)
        .into_decoder()
        .map_err(|error| format!("it could not be read: {error}"))?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut picture = DynamicImage::from_decoder(decoder)
        .map_err(|error| format!("it could not be read: {error}"))?;
    picture.apply_orientation(orientation);
    let (width, height) = (picture.width(), picture.height());
    if width == 0 || height == 0 {
        return Err("it has no pixels".into());
    }

    let too_large = width.max(height) > MAX_SIDE;
    let turned = orientation != Orientation::NoTransforms;
    let (mime, data, sent, note) = match kept {
        Some(mime) if !too_large && !turned && bytes.len() <= MAX_BYTES => {
            (mime, Arc::<[u8]>::from(bytes), (width, height), None)
        }
        _ => {
            let fitted = if too_large {
                picture.resize(MAX_SIDE, MAX_SIDE, FilterType::Lanczos3)
            } else {
                picture.clone()
            };
            // A photograph stays a JPEG; the rest becomes a PNG, unless that
            // is too large to send.
            let photo = format == ImageFormat::Jpeg;
            let (mime, mut data) = if photo {
                ("image/jpeg", encode_jpeg(&fitted)?)
            } else {
                ("image/png", encode_png(&fitted)?)
            };
            let (mut mime, mut was) = (mime, None);
            if data.len() > MAX_BYTES && !photo {
                (mime, data) = ("image/jpeg", encode_jpeg(&fitted)?);
                was = Some("made a JPEG to fit 20 MB");
            }
            if data.len() > MAX_BYTES {
                return Err(format!(
                    "even made smaller it is {}; a prompt takes pictures of up to {}",
                    size(data.len()),
                    size(MAX_BYTES)
                ));
            }
            let kind = match format {
                ImageFormat::Tiff => "TIFF".to_owned(),
                other => other
                    .extensions_str()
                    .first()
                    .map(|extension| extension.to_uppercase())
                    .unwrap_or_default(),
            };
            let note = match (kept.is_none(), too_large) {
                (true, true) => format!("converted from {kind}, scaled down from {width}×{height}"),
                (true, false) => format!("converted from {kind}"),
                (false, true) => format!("scaled down from {width}×{height}"),
                (false, false) => "turned upright".to_owned(),
            };
            let note = match was {
                Some(was) => format!("{note}, {was}"),
                None => note,
            };
            let sent = (fitted.width(), fitted.height());
            (mime, Arc::from(data), sent, Some(note))
        }
    };
    Ok(Prepared {
        mime,
        data,
        width: sent.0,
        height: sent.1,
        note,
        thumb: render(&picture, THUMB),
        large: render(&picture, LARGE),
    })
}

fn encode_png(picture: &DynamicImage) -> Result<Vec<u8>, String> {
    let mut out = Cursor::new(Vec::new());
    picture
        .write_to(&mut out, ImageFormat::Png)
        .map_err(|error| format!("it could not be converted: {error}"))?;
    Ok(out.into_inner())
}

fn encode_jpeg(picture: &DynamicImage) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let rgb = picture.to_rgb8();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 88)
        .encode_image(&rgb)
        .map_err(|error| format!("it could not be converted: {error}"))?;
    Ok(out)
}

/// The picture at most `side` pixels on its longer side, ready to paint.
fn render(picture: &DynamicImage, side: u32) -> Arc<RenderImage> {
    let small = if picture.width().max(picture.height()) > side {
        picture.resize(side, side, FilterType::Triangle)
    } else {
        picture.clone()
    };
    let mut pixels = small.into_rgba8();
    // GPUI paints BGRA.
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Arc::new(RenderImage::new(vec![Frame::new(pixels)]))
}

/// Decodes a picture a message brought, for the transcript: its thumbnail
/// and its larger picture, or nothing when it cannot be read.
pub(crate) fn decode(bytes: &[u8]) -> Option<(Arc<RenderImage>, Arc<RenderImage>, u32, u32)> {
    let picture = image::load_from_memory(bytes).ok()?;
    let (width, height) = (picture.width(), picture.height());
    Some((
        render(&picture, THUMB),
        render(&picture, LARGE),
        width,
        height,
    ))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    /// A picture of `width` by `height` in `format`, a gradient so that it
    /// does not compress to nothing.
    pub(crate) fn picture(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
        let buffer = ImageBuffer::from_fn(width, height, |x, y| {
            Rgba([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8, 255])
        });
        let mut out = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(buffer)
            .write_to(&mut out, format)
            .expect("an encodable picture");
        out.into_inner()
    }

    #[test]
    fn pictures_models_take_go_as_they_are() {
        let png = picture(64, 48, ImageFormat::Png);
        let ready = prepare(&png).expect("a PNG");
        assert_eq!(ready.mime, "image/png");
        assert_eq!(&*ready.data, png.as_slice(), "the bytes are not touched");
        assert_eq!((ready.width, ready.height), (64, 48));
        assert_eq!(ready.note, None);
        assert_eq!(ready.thumb.size(0).width.0, 64);
        assert_eq!(ready.describe(), format!("64×48 PNG · {}", size(png.len())));
    }

    #[test]
    fn bmp_and_tiff_are_converted_and_large_pictures_scaled_down() {
        let bmp = prepare(&picture(40, 30, ImageFormat::Bmp)).expect("a BMP");
        assert_eq!(bmp.mime, "image/png");
        assert_eq!(bmp.note.as_deref(), Some("converted from BMP"));
        assert!(image::load_from_memory_with_format(&bmp.data, ImageFormat::Png).is_ok());

        let tiff = prepare(&picture(2400, 1200, ImageFormat::Tiff)).expect("a TIFF");
        assert_eq!(tiff.mime, "image/png");
        assert_eq!((tiff.width, tiff.height), (2000, 1000));
        assert_eq!(
            tiff.note.as_deref(),
            Some("converted from TIFF, scaled down from 2400×1200")
        );

        let photo = prepare(&picture(3000, 2000, ImageFormat::Jpeg)).expect("a JPEG");
        assert_eq!(photo.mime, "image/jpeg", "a photograph stays a JPEG");
        assert_eq!((photo.width, photo.height), (2000, 1333));
        assert_eq!(photo.note.as_deref(), Some("scaled down from 3000×2000"));
        // The thumbnail and the larger picture keep the proportions.
        let thumb = photo.thumb.size(0);
        assert_eq!((thumb.width.0, thumb.height.0), (192, 128));
        assert_eq!(photo.large.size(0).width.0, 1200);
    }

    #[test]
    fn what_is_no_picture_says_why() {
        let error = prepare(b"plain text, not a picture").expect_err("no picture");
        assert!(error.contains("not a picture"), "{error}");
        let huge = vec![0u8; MAX_SOURCE + 1];
        assert!(prepare(&huge).expect_err("too large").contains("MB"));
    }

    #[test]
    fn labels_are_numbered_and_found_as_the_cockpits_write_them() {
        assert_eq!(label(3), "[Image 3]");
        assert_eq!(
            label_numbers("see [Image 2] and [Image 12], not [Image x] or [Image 0]"),
            [2, 12]
        );
        assert!(is_picture_path(Path::new("/tmp/Shot.PNG")));
        assert!(is_picture_path(Path::new("scan.tiff")));
        assert!(!is_picture_path(Path::new("notes.md")));
        assert_eq!(size(512), "512 B");
        assert_eq!(size(240 * 1024), "240 KB");
        assert_eq!(size(3 * 1024 * 1024 + 512 * 1024), "3.5 MB");
    }
}
