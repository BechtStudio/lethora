//! Image quality at PDF export ([`ImageQuality`]): raster images are replaced
//! by downscaled copies, sized for their largest placement at the quality's
//! resolution. Pages, positions and sizes stay as laid out; only the bytes
//! of the images in the PDF change. Vector images (SVG, PDF) are untouched.

use std::collections::HashMap;
use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::imageops::FilterType;
use image::{ColorType, DynamicImage};
use typst::foundations::{Bytes, Smart};
use typst::layout::{Frame, FrameItem, GroupItem, Size, Transform};
use typst::model::Document as _;
use typst::syntax::Span;
use typst::visualize::{ExchangeFormat, Image, ImageKind, RasterFormat, RasterImage};
use typst_layout::PagedDocument;

use crate::layout::ImageQuality;

/// JPEG quality of re-encoded JPEGs, per image quality: high enough that
/// photos show no artefacts in print, smaller for the screen and email.
pub const JPEG_QUALITY_PRINT: u8 = 90;
pub const JPEG_QUALITY_SCREEN: u8 = 85;
pub const JPEG_QUALITY_EMAIL: u8 = 75;

/// Images at most this much larger than needed are left as they are.
const SLACK: f64 = 1.1;

fn jpeg_quality(quality: ImageQuality) -> u8 {
    match quality {
        ImageQuality::Original | ImageQuality::Print => JPEG_QUALITY_PRINT,
        ImageQuality::Screen => JPEG_QUALITY_SCREEN,
        ImageQuality::Email => JPEG_QUALITY_EMAIL,
    }
}

/// Calls `f` with every image of `frame` (placed by `ts`): the image, its
/// size, its span and its placement.
pub(crate) fn visit(
    frame: &Frame,
    ts: Transform,
    f: &mut impl FnMut(&Image, Size, Span, Transform),
) {
    for (pos, item) in frame.items() {
        let ts = ts.pre_concat(Transform::translate(pos.x, pos.y));
        match item {
            FrameItem::Group(group) => visit(&group.frame, ts.pre_concat(group.transform), f),
            FrameItem::Image(image, size, span) => f(image, *size, *span, ts),
            _ => {}
        }
    }
}

/// `document` with its raster images at the quality `quality_of` gives for
/// their span; `None` if no image changes. An image is only made smaller,
/// never larger, and only if it is more than [`SLACK`] times the pixels its
/// largest placement needs. Each image is converted once per quality and the
/// copy shared by all its placements, so the PDF holds it once.
// Images are keys: their only interior mutability is a cache of their hash.
#[allow(clippy::mutable_key_type)]
pub fn downscale(
    document: &PagedDocument,
    quality_of: impl Fn(Span) -> ImageQuality,
) -> Option<PagedDocument> {
    // The scale each image needs at each quality: its largest placement's.
    let mut needed: HashMap<(Image, ImageQuality), f64> = HashMap::new();
    for page in document.pages() {
        visit(
            &page.frame,
            Transform::identity(),
            &mut |image, size, span, ts| {
                let ImageKind::Raster(raster) = image.kind() else {
                    return;
                };
                let quality = quality_of(span);
                let Some(dpi) = quality.dpi() else {
                    return;
                };
                // Placed size in inches: the item's size scaled by the transform.
                let w = size.x.to_pt() * ts.sx.get().hypot(ts.ky.get()) / 72.0;
                let h = size.y.to_pt() * ts.kx.get().hypot(ts.sy.get()) / 72.0;
                let scale =
                    (w * dpi / f64::from(raster.width())).max(h * dpi / f64::from(raster.height()));
                let entry = needed.entry((image.clone(), quality)).or_insert(0.0);
                *entry = entry.max(scale);
            },
        );
    }
    let replaced: HashMap<(Image, ImageQuality), Image> = needed
        .into_iter()
        .filter(|&(_, scale)| scale.is_finite() && scale * SLACK < 1.0)
        .filter_map(|((image, quality), scale)| {
            let ImageKind::Raster(raster) = image.kind() else {
                return None;
            };
            let small = shrink(raster, scale, jpeg_quality(quality))?;
            let new = Image::new(small, image.alt().map(Into::into), image.scaling());
            Some(((image, quality), new))
        })
        .collect();
    if replaced.is_empty() {
        return None;
    }
    let pages = document
        .pages()
        .iter()
        .map(|page| {
            let mut page = page.clone();
            if let Some(frame) = rebuild(&page.frame, &quality_of, &replaced) {
                page.frame = frame;
            }
            page
        })
        .collect();
    Some(PagedDocument::new(pages, document.info().clone()))
}

/// `frame` with the images in `replaced`; `None` if it has none of them.
#[allow(clippy::mutable_key_type)]
fn rebuild(
    frame: &Frame,
    quality_of: &impl Fn(Span) -> ImageQuality,
    replaced: &HashMap<(Image, ImageQuality), Image>,
) -> Option<Frame> {
    let mut items: Option<Vec<_>> = None;
    for (i, (pos, item)) in frame.items().enumerate() {
        let new = match item {
            FrameItem::Group(group) => rebuild(&group.frame, quality_of, replaced).map(|f| {
                FrameItem::Group(GroupItem {
                    frame: f,
                    ..group.clone()
                })
            }),
            FrameItem::Image(image, size, span) if matches!(image.kind(), ImageKind::Raster(_)) => {
                replaced
                    .get(&(image.clone(), quality_of(*span)))
                    .map(|new| FrameItem::Image(new.clone(), *size, *span))
            }
            _ => None,
        };
        match (new, &mut items) {
            (Some(new), None) => {
                // The first change: copy the items before it.
                let mut list: Vec<_> = frame.items().take(i).cloned().collect();
                list.push((*pos, new));
                items = Some(list);
            }
            (Some(new), Some(list)) => list.push((*pos, new)),
            (None, Some(list)) => list.push((*pos, item.clone())),
            (None, None) => {}
        }
    }
    let mut out = Frame::new(frame.size(), frame.kind());
    if frame.has_baseline() {
        out.set_baseline(frame.baseline());
    }
    out.push_multiple(items?);
    Some(out)
}

/// `raster` resampled to `scale` (< 1) and re-encoded: JPEG stays JPEG at
/// `jpeg_quality`, everything else becomes PNG (lossless, alpha kept). The
/// pixels are the decoded ones, already turned by the EXIF orientation (the
/// copy has none). `None` if that is not smaller, fails, or the colour
/// profile would no longer fit the pixels.
fn shrink(raster: &RasterImage, scale: f64, jpeg_quality: u8) -> Option<RasterImage> {
    let jpeg = raster.format() == RasterFormat::Exchange(ExchangeFormat::Jpg);
    let src = raster.dynamic();
    let size = |n: u32| ((f64::from(n) * scale).round() as u32).max(1);
    let mut small = src.resize_exact(size(src.width()), size(src.height()), FilterType::Lanczos3);
    if jpeg && !matches!(small.color(), ColorType::L8 | ColorType::Rgb8) {
        small = DynamicImage::ImageRgb8(small.to_rgb8());
    }
    let icc = match raster.icc() {
        Some(icc) if !icc_fits(icc, small.color()) => return None,
        Some(icc) => Smart::Custom(icc.clone()),
        None => Smart::Auto,
    };
    let mut bytes = Vec::new();
    let out = Cursor::new(&mut bytes);
    let (encoded, format) = if jpeg {
        let encoder = JpegEncoder::new_with_quality(out, jpeg_quality);
        (small.write_with_encoder(encoder), ExchangeFormat::Jpg)
    } else {
        (
            small.write_with_encoder(PngEncoder::new(out)),
            ExchangeFormat::Png,
        )
    };
    if encoded.is_err() || bytes.len() >= raster.data().len() {
        return None;
    }
    RasterImage::new(Bytes::new(bytes), format, icc).ok()
}

/// Whether ICC profile `icc` describes pixels of colour type `color` (a
/// CMYK profile does not fit the RGB pixels a CMYK JPEG decodes to).
fn icc_fits(icc: &[u8], color: ColorType) -> bool {
    let space = icc.get(16..20);
    if color.has_color() {
        space == Some(b"RGB ")
    } else {
        space == Some(b"GRAY")
    }
}
