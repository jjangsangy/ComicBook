//! PDF output.
//!
//! [`build_pdf`] is the Rust counterpart of KCC's `buildPDF` (see docs/output.md):
//! one page per processed image at its native pixel size, with the book title and
//! first author in the document information dictionary. KCC streams the images
//! through PyMuPDF; here each image is embedded directly as an XObject — a JPEG
//! via `DCTDecode` (no recompression), anything else decoded and `FlateDecode`d.
//!
//! The document skeleton is written with the pure-Rust `pdf-writer` crate, which
//! also emits the cross-reference table and trailer (see docs/dependencies.md).

use std::borrow::Cow;
use std::io::Write;
use std::path::Path;

use anyhow::{bail, Context, Result};
use flate2::write::ZlibEncoder;
use flate2::Compression;
use image::{ColorType, DynamicImage, ImageDecoder};
use pdf_writer::{Content, Filter, Name, Pdf, Rect, Ref, TextStr};

use crate::ebook::model::{EncodedPage, MediaType};
use crate::ebook::processing::ProcessedBook;
use crate::ebook::PreparedBook;

/// Build the PDF for `book` and write it to `dest`.
///
/// `title` is the tome's title (the base title, or `base [i/n]` when the book was
/// split into tomes).
pub fn build_pdf(
    dest: &Path,
    book: &ProcessedBook,
    prepared: &PreparedBook,
    title: &str,
) -> Result<()> {
    // KCC writes the cover into `OEBPS/Images/cover.jpg` before its walk only when
    // it was smart-cropped or came from a sibling `Covers/` override, in which case
    // the sorted walk makes it the first PDF page (see docs/output.md).
    let mut pages: Vec<&EncodedPage> = Vec::new();
    if let Some(cover) = &book.cover {
        if cover.smart_cropped || prepared.cover_override.is_some() {
            pages.push(&cover.page);
        }
    }
    for chapter in &book.chapters {
        for page in &chapter.pages {
            pages.push(page);
        }
    }
    if pages.is_empty() {
        bail!("Nothing to write into the PDF: the book has no pages");
    }

    let mut pdf = Pdf::new();
    let catalog_id = Ref::new(1);
    let page_tree_id = Ref::new(2);
    let info_id = Ref::new(3);
    let mut next = Ref::new(4);

    pdf.catalog(catalog_id).pages(page_tree_id);

    let mut page_ids = Vec::with_capacity(pages.len());
    for page in &pages {
        let page_id = next.bump();
        let content_id = next.bump();
        let image_id = next.bump();
        page_ids.push(page_id);

        let image = PdfImage::encode(page)?;
        // Each page has its own `/Resources`, so a constant XObject name is fine.
        let name = Name(b"Im1");
        let size = page.size;
        let (width, height) = (size.width as f32, size.height as f32);

        {
            let mut xobject = pdf.image_xobject(image_id, &image.data);
            xobject.filter(image.filter);
            xobject.width(size.width as i32);
            xobject.height(size.height as i32);
            match image.color_space {
                ColorSpace::Gray => xobject.color_space().device_gray(),
                ColorSpace::Rgb => xobject.color_space().device_rgb(),
            }
            xobject.bits_per_component(8);
        }

        // Map the 1x1 image XObject onto the whole page (KCC's `insert_image`).
        let mut content = Content::new();
        content.save_state();
        content.transform([width, 0.0, 0.0, height, 0.0, 0.0]);
        content.x_object(name);
        content.restore_state();
        pdf.stream(content_id, &content.finish());

        let mut page_writer = pdf.page(page_id);
        page_writer.parent(page_tree_id);
        page_writer.media_box(Rect::new(0.0, 0.0, width, height));
        page_writer.resources().x_objects().pair(name, image_id);
        page_writer.contents(content_id);
    }

    pdf.pages(page_tree_id)
        .kids(page_ids.iter().copied())
        .count(pages.len() as i32);

    {
        let mut info = pdf.document_info(info_id);
        info.title(TextStr(title));
        if let Some(author) = prepared.metadata.authors.first() {
            info.author(TextStr(author.as_str()));
        }
    }

    let bytes = pdf.finish();
    std::fs::write(dest, bytes).with_context(|| format!("Failed to write {}", dest.display()))?;
    Ok(())
}

/// An image ready to be embedded as an image XObject.
struct PdfImage<'a> {
    filter: Filter,
    /// The (possibly compressed) stream payload: borrowed from the page when its
    /// JPEG can be embedded verbatim, owned when it had to be re-encoded.
    data: Cow<'a, [u8]>,
    /// One component (`DeviceGray`) or three (`DeviceRGB`).
    color_space: ColorSpace,
}

/// The colour space a PDF image XObject is written in (REFACTOR.md A16).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColorSpace {
    Gray,
    Rgb,
}

impl<'a> PdfImage<'a> {
    /// Prepare one processed page for embedding.
    ///
    /// A JPEG whose header declares a gray or RGB 8-bit image is embedded verbatim
    /// through `DCTDecode`. Anything else (PNG/GIF/WebP, a `--no-processing` CMYK
    /// JPEG, an exotic colour type) is decoded and its raw samples `FlateDecode`d.
    fn encode(page: &'a EncodedPage) -> Result<PdfImage<'a>> {
        if page.media_type == MediaType::Jpeg {
            if let Some(color_space) = jpeg_color_space(&page.bytes) {
                return Ok(PdfImage {
                    filter: Filter::DctDecode,
                    // Borrow the page's bytes rather than copying the whole JPEG
                    // into the writer's input buffer.
                    data: Cow::Borrowed(&page.bytes),
                    color_space,
                });
            }
        }

        let image = image::load_from_memory(&page.bytes)
            .with_context(|| format!("Failed to decode {} for the PDF", page.name))?;
        raw_image(&image)
    }
}

/// Encode decoded pixels as a `FlateDecode`d 8-bit gray or RGB stream.
fn raw_image(image: &DynamicImage) -> Result<PdfImage<'static>> {
    let color_space = decoded_color_space(image.color());
    let data = match color_space {
        ColorSpace::Gray => Cow::Owned(deflate(image.to_luma8().as_raw())?),
        ColorSpace::Rgb => Cow::Owned(deflate(image.to_rgb8().as_raw())?),
    };
    Ok(PdfImage {
        filter: Filter::FlateDecode,
        data,
        color_space,
    })
}

/// The PDF colour space for a decoded image, chosen from its `ColorType`.
///
/// `image::ColorType` is `#[non_exhaustive]`, so the final arm is required by the
/// compiler; it preserves the old `is_gray` fallback (re-encode as RGB).
fn decoded_color_space(color: ColorType) -> ColorSpace {
    match color {
        ColorType::L8 | ColorType::La8 | ColorType::L16 | ColorType::La16 => ColorSpace::Gray,
        _ => ColorSpace::Rgb,
    }
}

/// The colour space of a JPEG we can embed verbatim, or `None` to re-encode.
///
/// Only the header is read, so a JPEG page never costs a full decode just to learn
/// its colour space. `ColorType` is `#[non_exhaustive]`; anything other than an
/// 8-bit gray/RGB JPEG (CMYK, 16-bit, …) is re-encoded.
fn jpeg_color_space(bytes: &[u8]) -> Option<ColorSpace> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let decoder = reader.into_decoder().ok()?;
    match decoder.color_type() {
        ColorType::L8 => Some(ColorSpace::Gray),
        ColorType::Rgb8 => Some(ColorSpace::Rgb),
        _ => None,
    }
}

/// zlib-compress raw image samples for the `FlateDecode` filter.
fn deflate(raw: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(raw).context("PDF deflate failed")?;
    encoder.finish().context("PDF deflate failed")
}

#[cfg(test)]
mod tests {
    use super::*;

    use image::{ImageEncoder, Luma, Rgb, RgbImage};

    #[test]
    fn jpeg_color_space_reads_the_header() -> Result<()> {
        let mut rgb = Vec::new();
        let mut gray = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut rgb).write_image(
            RgbImage::from_pixel(4, 6, Rgb([1, 2, 3])).as_raw(),
            4,
            6,
            image::ExtendedColorType::Rgb8,
        )?;
        image::codecs::jpeg::JpegEncoder::new(&mut gray).write_image(
            image::GrayImage::from_pixel(4, 6, Luma([9])).as_raw(),
            4,
            6,
            image::ExtendedColorType::L8,
        )?;

        assert_eq!(jpeg_color_space(&rgb), Some(ColorSpace::Rgb));
        assert_eq!(jpeg_color_space(&gray), Some(ColorSpace::Gray));
        assert_eq!(jpeg_color_space(b"not a jpeg"), None);
        Ok(())
    }

    #[test]
    fn raw_pdf_pixels_keep_their_channel_count() -> Result<()> {
        let gray = raw_image(&DynamicImage::ImageLuma8(image::GrayImage::from_pixel(
            2,
            2,
            Luma([7]),
        )))?;
        assert_eq!(gray.color_space, ColorSpace::Gray);
        assert!(!gray.data.is_empty());

        let rgb = raw_image(&DynamicImage::ImageRgb8(RgbImage::from_pixel(
            2,
            2,
            Rgb([1, 2, 3]),
        )))?;
        assert_eq!(rgb.color_space, ColorSpace::Rgb);
        Ok(())
    }
}
