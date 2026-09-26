//! PDF output (Phase 7).
//!
//! [`build_pdf`] is the Rust counterpart of KCC's `buildPDF` (AGENTS.md §12.3):
//! one page per processed image at its native pixel size, with the book title and
//! first author in the document information dictionary. KCC streams the images
//! through PyMuPDF; here each image is embedded directly as an XObject — a JPEG
//! via `DCTDecode` (no recompression), anything else decoded and `FlateDecode`d.
//!
//! The document skeleton is written with the pure-Rust `pdf-writer` crate, which
//! also emits the cross-reference table and trailer (AGENTS.md §5.3/§7).

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
pub fn build_pdf(dest: &Path, book: &ProcessedBook, prepared: &PreparedBook) -> Result<()> {
    // KCC writes the cover into `OEBPS/Images/cover.jpg` before its walk only when
    // it was smart-cropped or came from a sibling `Covers/` override, in which case
    // the sorted walk makes it the first PDF page (AGENTS.md §12.3).
    let mut pages: Vec<&EncodedPage> = Vec::new();
    if let Some(cover) = &book.cover {
        if book.cover_smart_crop || prepared.cover_override.is_some() {
            pages.push(cover);
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
        let (width, height) = (page.width as f32, page.height as f32);

        {
            let mut xobject = pdf.image_xobject(image_id, &image.data);
            xobject.filter(image.filter);
            xobject.width(page.width as i32);
            xobject.height(page.height as i32);
            if image.gray {
                xobject.color_space().device_gray();
            } else {
                xobject.color_space().device_rgb();
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
        info.title(TextStr(prepared.metadata.title.as_str()));
        if let Some(author) = prepared.metadata.authors.first() {
            info.author(TextStr(author.as_str()));
        }
    }

    let bytes = pdf.finish();
    std::fs::write(dest, bytes).with_context(|| format!("Failed to write {}", dest.display()))?;
    Ok(())
}

/// An image ready to be embedded as an image XObject.
struct PdfImage {
    filter: Filter,
    /// The (possibly compressed) stream payload.
    data: Vec<u8>,
    /// One component (`DeviceGray`) versus three (`DeviceRGB`).
    gray: bool,
}

impl PdfImage {
    /// Prepare one processed page for embedding.
    ///
    /// A JPEG whose header declares a gray or RGB 8-bit image is embedded verbatim
    /// through `DCTDecode`. Anything else (PNG/GIF/WebP, a `--no-processing` CMYK
    /// JPEG, an exotic colour type) is decoded and its raw samples `FlateDecode`d.
    fn encode(page: &EncodedPage) -> Result<PdfImage> {
        if page.media_type == MediaType::Jpeg {
            if let Some(components) = jpeg_components(&page.bytes) {
                return Ok(PdfImage {
                    filter: Filter::DctDecode,
                    data: page.bytes.clone(),
                    gray: components == 1,
                });
            }
        }

        let image = image::load_from_memory(&page.bytes)
            .with_context(|| format!("Failed to decode {} for the PDF", page.name))?;
        raw_image(&image)
    }
}

/// Encode decoded pixels as a `FlateDecode`d 8-bit gray or RGB stream.
fn raw_image(image: &DynamicImage) -> Result<PdfImage> {
    if is_gray(image.color()) {
        let gray = image.to_luma8();
        Ok(PdfImage {
            filter: Filter::FlateDecode,
            data: deflate(gray.as_raw())?,
            gray: true,
        })
    } else {
        let rgb = image.to_rgb8();
        Ok(PdfImage {
            filter: Filter::FlateDecode,
            data: deflate(rgb.as_raw())?,
            gray: false,
        })
    }
}

/// Whether a decoded image is single-channel.
fn is_gray(color: ColorType) -> bool {
    matches!(
        color,
        ColorType::L8 | ColorType::La8 | ColorType::L16 | ColorType::La16
    )
}

/// The component count of a JPEG we can embed verbatim, or `None` to re-encode.
///
/// Only the header is read, so a JPEG page never costs a full decode just to learn
/// its colour space.
fn jpeg_components(bytes: &[u8]) -> Option<u8> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let decoder = reader.into_decoder().ok()?;
    match decoder.color_type() {
        ColorType::L8 => Some(1),
        ColorType::Rgb8 => Some(3),
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
    fn jpeg_components_reads_the_header() {
        let mut rgb = Vec::new();
        let mut gray = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut rgb)
            .write_image(
                RgbImage::from_pixel(4, 6, Rgb([1, 2, 3])).as_raw(),
                4,
                6,
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        image::codecs::jpeg::JpegEncoder::new(&mut gray)
            .write_image(
                image::GrayImage::from_pixel(4, 6, Luma([9])).as_raw(),
                4,
                6,
                image::ExtendedColorType::L8,
            )
            .unwrap();

        assert_eq!(jpeg_components(&rgb), Some(3));
        assert_eq!(jpeg_components(&gray), Some(1));
        assert_eq!(jpeg_components(b"not a jpeg"), None);
    }

    #[test]
    fn raw_pdf_pixels_keep_their_channel_count() {
        let gray = raw_image(&DynamicImage::ImageLuma8(image::GrayImage::from_pixel(
            2,
            2,
            Luma([7]),
        )))
        .unwrap();
        assert!(gray.gray);
        assert!(!gray.data.is_empty());

        let rgb = raw_image(&DynamicImage::ImageRgb8(RgbImage::from_pixel(
            2,
            2,
            Rgb([1, 2, 3]),
        )))
        .unwrap();
        assert!(!rgb.gray);
    }
}
