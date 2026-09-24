use comic_book::archive::{
    compress_archive, detect_archive_kind, detect_archive_kind_from_bytes, extract_archive,
    get_images_from_source, normalize_archive_path, parse_target_extension, read_archive_entries,
    safe_join, ArchiveKind, ArchiveWriter,
};
use comic_book::clamp::{remove_dir_all_force, Approach};
use comic_book::image_ops::{
    is_image_extension, is_image_file, resize_image_by_total_pixels, resize_image_by_width,
    save_image_as_webp, split_image_iterative,
};
use image::{DynamicImage, GenericImageView, Rgb, RgbImage};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

mod common;
use common::{run_clamp, run_convert};

// ===================================================================
// Image Operations Tests
// ===================================================================

#[test]
fn test_image_split_even() {
    // 100 x 200 = 20,000 pixels
    let img = DynamicImage::ImageRgb8(RgbImage::new(100, 200));
    // Threshold 6,000:
    // Split 1: two 100x100 (10,000 pixels each) -> both > 6,000
    // Split 2: each splits into two 100x50 (5,000 pixels each) -> all 4 < 6,000
    let pieces = split_image_iterative(img, 6000);
    assert_eq!(pieces.len(), 4);
    for piece in &pieces {
        let (w, h) = piece.dimensions();
        assert_eq!(w, 100);
        assert_eq!(h, 50);
        assert!((w as u64) * (h as u64) < 6000);
    }
}

#[test]
fn test_image_split_odd_height_and_ordering() {
    // Height 201: top half is 100, bottom half is 101.
    // Fill top pixel red, bottom pixel blue.
    let mut buf = RgbImage::new(100, 201);
    buf.put_pixel(0, 0, Rgb([255, 0, 0]));
    buf.put_pixel(0, 200, Rgb([0, 0, 255]));
    let img = DynamicImage::ImageRgb8(buf);

    let pieces = split_image_iterative(img, 15_000);
    assert_eq!(pieces.len(), 2);

    let (w1, h1) = pieces[0].dimensions();
    assert_eq!((w1, h1), (100, 100));
    // Top half must come first (reading order)
    assert_eq!(pieces[0].to_rgb8().get_pixel(0, 0), &Rgb([255, 0, 0]));

    let (w2, h2) = pieces[1].dimensions();
    assert_eq!((w2, h2), (100, 101));
    // Bottom half must come second
    assert_eq!(pieces[1].to_rgb8().get_pixel(0, 100), &Rgb([0, 0, 255]));
}

#[test]
fn test_image_split_already_under_threshold() {
    let img = DynamicImage::ImageRgb8(RgbImage::new(50, 50));
    let pieces = split_image_iterative(img, 5000);
    assert_eq!(pieces.len(), 1);
    assert_eq!(pieces[0].dimensions(), (50, 50));
}

#[test]
fn test_image_resize_total_pixels() {
    let img = DynamicImage::ImageRgb8(RgbImage::new(1000, 1000));
    let resized = resize_image_by_total_pixels(img, 250_000);
    let (w, h) = resized.dimensions();
    assert!((w as u64) * (h as u64) <= 250_000);
    assert_eq!(w, 500);
    assert_eq!(h, 500);
}

#[test]
fn test_image_resize_total_pixels_already_smaller() {
    let img = DynamicImage::ImageRgb8(RgbImage::new(200, 300));
    let resized = resize_image_by_total_pixels(img, 100_000);
    assert_eq!(resized.dimensions(), (200, 300));
}

#[test]
fn test_image_resize_max_width() {
    let img = DynamicImage::ImageRgb8(RgbImage::new(1200, 800));
    let resized = resize_image_by_width(img, 600);
    let (w, h) = resized.dimensions();
    assert_eq!(w, 600);
    assert_eq!(h, 400);
}

#[test]
fn test_image_resize_max_width_already_smaller() {
    let img = DynamicImage::ImageRgb8(RgbImage::new(400, 800));
    let resized = resize_image_by_width(img, 600);
    assert_eq!(resized.dimensions(), (400, 800));
}

#[test]
fn test_webp_save() {
    let tmp = tempdir().unwrap();
    let out_file = tmp.path().join("test.webp");
    let img = DynamicImage::ImageRgb8(RgbImage::new(50, 50));
    save_image_as_webp(&img, &out_file, 90.0).unwrap();
    assert!(out_file.exists());
    let decoded = image::open(&out_file).unwrap();
    assert_eq!(decoded.dimensions(), (50, 50));
}

// ===================================================================
// Extension and Format Detection Tests
// ===================================================================

#[test]
fn test_supported_and_unsupported_extensions() {
    // Supported
    assert!(is_image_extension("jpg"));
    assert!(is_image_extension("JPEG"));
    assert!(is_image_extension(".png"));
    assert!(is_image_extension(".webp"));
    assert!(is_image_extension(".TIFF"));
    assert!(is_image_extension(".tif"));
    assert!(is_image_extension(".bmp"));
    assert!(is_image_extension(".gif"));
    assert!(is_image_extension(".pgm"));

    assert!(is_image_file(Path::new("page.PNG")));
    assert!(is_image_file(Path::new("cover.jpeg")));

    // Unsupported (explicitly removed / not images)
    assert!(!is_image_extension("heic"));
    assert!(!is_image_extension("heif"));
    assert!(!is_image_extension("jxl"));
    assert!(!is_image_extension("avif"));
    assert!(!is_image_extension("pdf"));
    assert!(!is_image_extension("txt"));
    assert!(!is_image_file(Path::new("comic.heic")));
    assert!(!is_image_file(Path::new("comic.jxl")));
    assert!(!is_image_file(Path::new("comic.avif")));
}

#[test]
fn test_detect_archive_kind() {
    let tmp = tempdir().unwrap();

    // 1. Directory detection
    assert_eq!(
        detect_archive_kind(tmp.path()),
        Some(ArchiveKind::Directory)
    );

    // 2. Real CBZ (ZIP) detection
    let cbz_path = tmp.path().join("test.cbz");
    {
        let mut writer = ArchiveWriter::new(ArchiveKind::Cbz, &cbz_path).unwrap();
        writer.add_entry("page.txt", false, b"data").unwrap();
        writer.finish().unwrap();
    }
    assert_eq!(detect_archive_kind(&cbz_path), Some(ArchiveKind::Cbz));

    // Even if named .zip, .cbr, .unknown, or with no extension, magic bytes detect it as Cbz
    let misnamed_cbz = tmp.path().join("misnamed_as_cbr.cbr");
    fs::copy(&cbz_path, &misnamed_cbz).unwrap();
    assert_eq!(detect_archive_kind(&misnamed_cbz), Some(ArchiveKind::Cbz));

    let no_ext_cbz = tmp.path().join("archive_with_no_ext");
    fs::copy(&cbz_path, &no_ext_cbz).unwrap();
    assert_eq!(detect_archive_kind(&no_ext_cbz), Some(ArchiveKind::Cbz));

    // 3. Real CBR (RAR) detection
    let cbr_path = tmp.path().join("test.cbr");
    {
        let mut writer = ArchiveWriter::new(ArchiveKind::Cbr, &cbr_path).unwrap();
        writer.add_entry("page.txt", false, b"data").unwrap();
        writer.finish().unwrap();
    }
    assert_eq!(detect_archive_kind(&cbr_path), Some(ArchiveKind::Cbr));

    // Misnamed RAR file with .cbz extension detected as Cbr
    let misnamed_cbr = tmp.path().join("misnamed_as_cbz.cbz");
    fs::copy(&cbr_path, &misnamed_cbr).unwrap();
    assert_eq!(detect_archive_kind(&misnamed_cbr), Some(ArchiveKind::Cbr));

    // 4. Real CB7 (7z) detection
    let cb7_path = tmp.path().join("test.cb7");
    {
        let mut writer = ArchiveWriter::new(ArchiveKind::Cb7, &cb7_path).unwrap();
        writer.add_entry("page.txt", false, b"data").unwrap();
        writer.finish().unwrap();
    }
    assert_eq!(detect_archive_kind(&cb7_path), Some(ArchiveKind::Cb7));

    // 5. Real CBT (TAR) detection
    let cbt_path = tmp.path().join("test.cbt");
    {
        let mut writer = ArchiveWriter::new(ArchiveKind::Cbt, &cbt_path).unwrap();
        writer.add_entry("page.txt", false, b"data").unwrap();
        writer.finish().unwrap();
    }
    assert_eq!(detect_archive_kind(&cbt_path), Some(ArchiveKind::Cbt));

    // 6. Non-archive file with .cbz extension is NOT detected as an archive
    let fake_cbz = tmp.path().join("fake.cbz");
    fs::write(&fake_cbz, b"This is plain text, not a zip file").unwrap();
    assert_eq!(detect_archive_kind(&fake_cbz), None);

    // 7. Empty file is NOT detected as an archive
    let empty_file = tmp.path().join("empty.cbz");
    fs::write(&empty_file, b"").unwrap();
    assert_eq!(detect_archive_kind(&empty_file), None);

    // 8. Non-existent file returns None
    assert_eq!(
        detect_archive_kind(tmp.path().join("nonexistent.cbz")),
        None
    );

    // 9. Direct byte detection tests
    assert_eq!(
        detect_archive_kind_from_bytes(&[0x50, 0x4b, 0x03, 0x04, 0x00, 0x00]),
        Some(ArchiveKind::Cbz)
    );
    assert_eq!(
        detect_archive_kind_from_bytes(&[0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x00]),
        Some(ArchiveKind::Cbr)
    );
    assert_eq!(
        detect_archive_kind_from_bytes(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]),
        Some(ArchiveKind::Cb7)
    );
    assert_eq!(detect_archive_kind_from_bytes(b"not an archive"), None);
}

#[test]
fn test_parse_target_extension() {
    assert_eq!(
        parse_target_extension("cbz"),
        Some(("cbz", ArchiveKind::Cbz))
    );
    assert_eq!(
        parse_target_extension(".ZIP"),
        Some(("zip", ArchiveKind::Cbz))
    );
    assert_eq!(
        parse_target_extension("cb7"),
        Some(("cb7", ArchiveKind::Cb7))
    );
    assert_eq!(parse_target_extension("7z"), Some(("7z", ArchiveKind::Cb7)));
    assert_eq!(
        parse_target_extension("cbt"),
        Some(("cbt", ArchiveKind::Cbt))
    );
    assert_eq!(
        parse_target_extension(".tar"),
        Some(("tar", ArchiveKind::Cbt))
    );
    assert_eq!(
        parse_target_extension("cbr"),
        Some(("cbr", ArchiveKind::Cbr))
    );
    assert_eq!(
        parse_target_extension("rar"),
        Some(("rar", ArchiveKind::Cbr))
    );
    assert_eq!(
        parse_target_extension("dir"),
        Some(("dir", ArchiveKind::Directory))
    );
    assert_eq!(
        parse_target_extension(".DIR"),
        Some(("dir", ArchiveKind::Directory))
    );
    assert_eq!(parse_target_extension("directory"), None);
    assert_eq!(parse_target_extension("folder"), None);
    assert_eq!(parse_target_extension("pdf"), None);
}

// ===================================================================
// Archive Roundtrip Tests (CBZ, CBT, CB7, Directory)
// ===================================================================

#[test]
fn test_cbr_archive_roundtrip() {
    let tmp = tempdir().unwrap();
    let src_dir = tmp.path().join("source");
    fs::create_dir_all(&src_dir).unwrap();

    let names = ["page_10.png", "page_1.png", "page_2.png", "page_20.png"];
    for name in &names {
        let img = DynamicImage::ImageRgb8(RgbImage::new(10, 10));
        img.save(src_dir.join(name)).unwrap();
    }

    let cbr_path = tmp.path().join("chapter.cbr");
    compress_archive(ArchiveKind::Cbr, &src_dir, &cbr_path).unwrap();
    assert!(cbr_path.exists());

    let images = get_images_from_source(ArchiveKind::Cbr, &cbr_path).unwrap();
    assert_eq!(images.len(), 4);
    assert_eq!(images[0].0, "page_1.png");
    assert_eq!(images[1].0, "page_2.png");
    assert_eq!(images[2].0, "page_10.png");
    assert_eq!(images[3].0, "page_20.png");

    let extract_dir = tmp.path().join("extracted");
    extract_archive(ArchiveKind::Cbr, &cbr_path, &extract_dir).unwrap();
    assert!(extract_dir.join("page_1.png").exists());
}

#[test]
fn test_cbr_large_archive_many_entries() {
    let tmp = tempdir().unwrap();
    let cbr_path = tmp.path().join("large.cbr");
    let mut writer = ArchiveWriter::new(ArchiveKind::Cbr, &cbr_path).unwrap();
    for i in 0..300 {
        writer
            .add_entry(&format!("page_{:03}.txt", i), false, b"dummy data")
            .unwrap();
    }
    writer.finish().unwrap();
    assert!(cbr_path.exists());

    let mut count = 0;
    read_archive_entries(ArchiveKind::Cbr, &cbr_path, |_name, _is_dir, _data| {
        count += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(count, 300);
}

#[test]
fn test_convert_file_cbz_to_cbr() {
    let tmp = tempdir().unwrap();
    let src_dir = tmp.path().join("source");
    fs::create_dir_all(&src_dir).unwrap();

    let img = DynamicImage::ImageRgb8(RgbImage::new(10, 10));
    img.save(src_dir.join("01.png")).unwrap();

    let cbz_path = tmp.path().join("input.cbz");
    compress_archive(ArchiveKind::Cbz, &src_dir, &cbz_path).unwrap();

    run_convert(std::slice::from_ref(&cbz_path), "cbr").unwrap();

    let cbr_path = tmp.path().join("input.cbr");
    assert!(cbr_path.exists());

    let images = get_images_from_source(ArchiveKind::Cbr, &cbr_path).unwrap();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].0, "01.png");
}

#[test]
fn test_convert_file_cbr_to_cbz() {
    let tmp = tempdir().unwrap();
    let src_dir = tmp.path().join("source");
    fs::create_dir_all(&src_dir).unwrap();

    let img = DynamicImage::ImageRgb8(RgbImage::new(10, 10));
    img.save(src_dir.join("01.png")).unwrap();

    let cbr_path = tmp.path().join("input.cbr");
    compress_archive(ArchiveKind::Cbr, &src_dir, &cbr_path).unwrap();

    run_convert(std::slice::from_ref(&cbr_path), "cbz").unwrap();

    let cbz_path = tmp.path().join("input.cbz");
    assert!(cbz_path.exists());

    let images = get_images_from_source(ArchiveKind::Cbz, &cbz_path).unwrap();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].0, "01.png");
}

#[test]
fn test_cbz_archive_roundtrip() {
    let tmp = tempdir().unwrap();
    let src_dir = tmp.path().join("source");
    fs::create_dir_all(&src_dir).unwrap();

    // Intentionally out of order to verify natural sorting
    let names = ["page_10.png", "page_1.png", "page_2.png", "page_20.png"];
    for name in &names {
        let img = DynamicImage::ImageRgb8(RgbImage::new(10, 10));
        img.save(src_dir.join(name)).unwrap();
    }

    let cbz_path = tmp.path().join("chapter.cbz");
    compress_archive(ArchiveKind::Cbz, &src_dir, &cbz_path).unwrap();
    assert!(cbz_path.exists());

    let images = get_images_from_source(ArchiveKind::Cbz, &cbz_path).unwrap();
    assert_eq!(images.len(), 4);
    // Verified natural sort order: page_1, page_2, page_10, page_20
    assert_eq!(images[0].0, "page_1.png");
    assert_eq!(images[1].0, "page_2.png");
    assert_eq!(images[2].0, "page_10.png");
    assert_eq!(images[3].0, "page_20.png");

    let extract_dir = tmp.path().join("extracted");
    extract_archive(ArchiveKind::Cbz, &cbz_path, &extract_dir).unwrap();
    assert!(extract_dir.join("page_1.png").exists());
}

#[test]
fn test_cbt_archive_roundtrip() {
    let tmp = tempdir().unwrap();
    let src_dir = tmp.path().join("source");
    fs::create_dir_all(&src_dir).unwrap();

    for i in 1..=3 {
        let img = DynamicImage::ImageRgb8(RgbImage::new(10, 10));
        img.save(src_dir.join(format!("page_{:02}.png", i)))
            .unwrap();
    }

    let cbt_path = tmp.path().join("chapter.cbt");
    compress_archive(ArchiveKind::Cbt, &src_dir, &cbt_path).unwrap();
    assert!(cbt_path.exists());

    let images = get_images_from_source(ArchiveKind::Cbt, &cbt_path).unwrap();
    assert_eq!(images.len(), 3);
    assert_eq!(images[0].0, "page_01.png");

    let extract_dir = tmp.path().join("extracted_tar");
    extract_archive(ArchiveKind::Cbt, &cbt_path, &extract_dir).unwrap();
    assert!(extract_dir.join("page_01.png").exists());
}

#[test]
fn test_cb7_archive_roundtrip() {
    let tmp = tempdir().unwrap();
    let src_dir = tmp.path().join("source");
    fs::create_dir_all(&src_dir).unwrap();

    let img = DynamicImage::ImageRgb8(RgbImage::new(15, 15));
    img.save(src_dir.join("p01.png")).unwrap();

    let cb7_path = tmp.path().join("chapter.cb7");
    compress_archive(ArchiveKind::Cb7, &src_dir, &cb7_path).unwrap();
    assert!(cb7_path.exists());

    let images = get_images_from_source(ArchiveKind::Cb7, &cb7_path).unwrap();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].0, "p01.png");

    let extract_dir = tmp.path().join("extracted_7z");
    extract_archive(ArchiveKind::Cb7, &cb7_path, &extract_dir).unwrap();
    assert!(extract_dir.join("p01.png").exists());
}

#[test]
fn test_cb7_speed_benchmark() {
    use std::time::Instant;
    let data = vec![0xABu8; 10 * 1024 * 1024]; // 10 MB
    let tmp = tempdir().unwrap();
    let cb7_path = tmp.path().join("bench.cb7");
    let mut writer = ArchiveWriter::new(ArchiveKind::Cb7, &cb7_path).unwrap();
    let start = Instant::now();
    writer.add_entry("large_page.jpg", false, &data).unwrap();
    writer.finish().unwrap();
    let elapsed = start.elapsed();
    println!("10MB archive write with COPY elapsed: {:?}", elapsed);
    assert!(
        elapsed.as_millis() < 1000,
        "Writing 10MB with COPY should be fast (< 1s), took {:?}",
        elapsed
    );
    // Stored/uncompressed files will be at least the uncompressed data size
    let file_size = cb7_path.metadata().unwrap().len();
    assert!(file_size >= data.len() as u64);

    // Verify it extracts back perfectly
    let extract_dir = tmp.path().join("extracted");
    extract_archive(ArchiveKind::Cb7, &cb7_path, &extract_dir).unwrap();
    let extracted_data = fs::read(extract_dir.join("large_page.jpg")).unwrap();
    assert_eq!(extracted_data, data);
}

#[test]
fn test_directory_archive_roundtrip() {
    let tmp = tempdir().unwrap();
    let dir_chapter = tmp.path().join("loose_images");
    fs::create_dir_all(&dir_chapter).unwrap();

    for i in 1..=2 {
        let img = DynamicImage::ImageRgb8(RgbImage::new(10, 10));
        img.save(dir_chapter.join(format!("scan_{}.png", i)))
            .unwrap();
    }

    let images = get_images_from_source(ArchiveKind::Directory, &dir_chapter).unwrap();
    assert_eq!(images.len(), 2);
    assert_eq!(images[0].0, "scan_1.png");
    assert_eq!(images[1].0, "scan_2.png");
}

// ===================================================================
// Convert Command Tests
// ===================================================================

#[test]
fn test_convert_command_cbz_to_cbt() {
    let tmp = tempdir().unwrap();
    let work_dir = tmp.path().join("comics");
    fs::create_dir_all(&work_dir).unwrap();

    let img_dir = tmp.path().join("raw");
    fs::create_dir_all(&img_dir).unwrap();
    let img = DynamicImage::ImageRgb8(RgbImage::new(20, 20));
    img.save(img_dir.join("p1.jpg")).unwrap();

    let cbz_path = work_dir.join("issue.cbz");
    compress_archive(ArchiveKind::Cbz, &img_dir, &cbz_path).unwrap();

    run_convert(std::slice::from_ref(&work_dir), "cbt").unwrap();
    let cbt_path = work_dir.join("issue.cbt");
    assert!(cbt_path.exists());
}

#[test]
fn test_convert_command_skip_same_format() {
    let tmp = tempdir().unwrap();
    let work_dir = tmp.path().join("comics");
    fs::create_dir_all(&work_dir).unwrap();

    let img_dir = tmp.path().join("raw");
    fs::create_dir_all(&img_dir).unwrap();
    let img = DynamicImage::ImageRgb8(RgbImage::new(10, 10));
    img.save(img_dir.join("p1.png")).unwrap();

    let cbz_path = work_dir.join("issue.cbz");
    compress_archive(ArchiveKind::Cbz, &img_dir, &cbz_path).unwrap();

    // Converting CBZ to ZIP (same format: ArchiveKind::Cbz) should skip
    run_convert(std::slice::from_ref(&work_dir), "zip").unwrap();
    // issue.zip should not be created because it's skipped as already a zip format
    assert!(!work_dir.join("issue.zip").exists());
}

#[test]
fn test_convert_file_cbz_to_cbt() {
    let tmp = tempdir().unwrap();
    let img_dir = tmp.path().join("raw");
    fs::create_dir_all(&img_dir).unwrap();
    let img = DynamicImage::ImageRgb8(RgbImage::new(15, 15));
    img.save(img_dir.join("page1.png")).unwrap();

    let src_cbz = tmp.path().join("input.cbz");
    compress_archive(ArchiveKind::Cbz, &img_dir, &src_cbz).unwrap();

    // Pass the cbz file directly to convert with --to cbt
    let paths = vec![src_cbz.clone()];
    run_convert(&paths, "cbt").unwrap();

    let expected_cbt = tmp.path().join("input.cbt");
    assert!(expected_cbt.exists());
    let images = get_images_from_source(ArchiveKind::Cbt, &expected_cbt).unwrap();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].0, "page1.png");
}

#[test]
fn test_convert_file_cbt_to_cb7() {
    let tmp = tempdir().unwrap();
    let img_dir = tmp.path().join("raw");
    fs::create_dir_all(&img_dir).unwrap();
    let img = DynamicImage::ImageRgb8(RgbImage::new(15, 15));
    img.save(img_dir.join("page1.png")).unwrap();

    let src_cbt = tmp.path().join("input.cbt");
    compress_archive(ArchiveKind::Cbt, &img_dir, &src_cbt).unwrap();

    let paths = vec![src_cbt.clone()];
    run_convert(&paths, "cb7").unwrap();

    let expected_cb7 = tmp.path().join("input.cb7");
    assert!(expected_cb7.exists());
    let images = get_images_from_source(ArchiveKind::Cb7, &expected_cb7).unwrap();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].0, "page1.png");
}

#[test]
fn test_convert_file_cb7_to_cbz() {
    let tmp = tempdir().unwrap();
    let img_dir = tmp.path().join("raw");
    fs::create_dir_all(&img_dir).unwrap();
    let img = DynamicImage::ImageRgb8(RgbImage::new(15, 15));
    img.save(img_dir.join("page1.png")).unwrap();

    let src_cb7 = tmp.path().join("input.cb7");
    compress_archive(ArchiveKind::Cb7, &img_dir, &src_cb7).unwrap();

    let paths = vec![src_cb7.clone()];
    run_convert(&paths, "cbz").unwrap();

    let expected_cbz = tmp.path().join("input.cbz");
    assert!(expected_cbz.exists());
    let images = get_images_from_source(ArchiveKind::Cbz, &expected_cbz).unwrap();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].0, "page1.png");
}

#[test]
fn test_convert_file_cb7_to_cbr() {
    let tmp = tempdir().unwrap();
    let img_dir = tmp.path().join("raw");
    let sub_dir = img_dir.join("chapter1");
    fs::create_dir_all(&sub_dir).unwrap();
    let img = DynamicImage::ImageRgb8(RgbImage::new(15, 15));
    img.save(sub_dir.join("page1.png")).unwrap();

    let src_cb7 = tmp.path().join("input.cb7");
    compress_archive(ArchiveKind::Cb7, &img_dir, &src_cb7).unwrap();

    let paths = vec![src_cb7.clone()];
    run_convert(&paths, "cbr").unwrap();

    let expected_cbr = tmp.path().join("input.cbr");
    assert!(expected_cbr.exists());
    let images = get_images_from_source(ArchiveKind::Cbr, &expected_cbr).unwrap();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].0, "page1.png");

    let extract_dir = tmp.path().join("extracted");
    extract_archive(ArchiveKind::Cbr, &expected_cbr, &extract_dir).unwrap();
    assert!(
        extract_dir.join("chapter1").join("page1.png").exists()
            || extract_dir.join("page1.png").exists()
    );

    // Verify RAR header attributes and names
    let mut arc = unrar::Archive::new(&expected_cbr)
        .open_for_processing()
        .unwrap();
    let mut count = 0;
    while let Some(h) = arc.read_header().unwrap() {
        count += 1;
        assert_eq!(h.entry().file_attr, 0o100644);
        assert!(!h.entry().is_directory());
        arc = h.skip().unwrap();
    }
    assert_eq!(count, 1);
}

#[test]
fn test_convert_multiple_files_with_to() {
    let tmp = tempdir().unwrap();
    let img_dir = tmp.path().join("raw");
    fs::create_dir_all(&img_dir).unwrap();
    let img = DynamicImage::ImageRgb8(RgbImage::new(10, 10));
    img.save(img_dir.join("p.png")).unwrap();

    let src1 = tmp.path().join("issue1.cbz");
    let src2 = tmp.path().join("issue2.cb7");
    compress_archive(ArchiveKind::Cbz, &img_dir, &src1).unwrap();
    compress_archive(ArchiveKind::Cb7, &img_dir, &src2).unwrap();

    let paths = vec![src1, src2];
    run_convert(&paths, "cbt").unwrap();

    assert!(tmp.path().join("issue1.cbt").exists());
    assert!(tmp.path().join("issue2.cbt").exists());
}

#[test]
fn test_convert_empty_paths_error() {
    let paths: Vec<PathBuf> = Vec::new();
    assert!(run_convert(&paths, "cbz").is_err());
}

#[test]
fn test_convert_unsupported_target_error() {
    let tmp = tempdir().unwrap();
    let dummy_file = tmp.path().join("dummy.cbz");
    fs::write(&dummy_file, b"content").unwrap();

    let paths = vec![dummy_file];
    assert!(run_convert(&paths, "unsupported_ext").is_err());
}

#[test]
fn test_convert_archive_to_directory() {
    let tmp = tempdir().unwrap();
    let raw_dir = tmp.path().join("raw");
    fs::create_dir_all(&raw_dir).unwrap();
    fs::write(raw_dir.join("01.jpg"), b"page 1 bytes").unwrap();
    fs::write(raw_dir.join("02.png"), b"page 2 bytes").unwrap();

    let cbz_path = tmp.path().join("chapter1.cbz");
    compress_archive(ArchiveKind::Cbz, &raw_dir, &cbz_path).unwrap();

    // Convert archive into a directory
    run_convert(&[cbz_path], "dir").unwrap();

    let extracted_dir = tmp.path().join("chapter1");
    assert!(extracted_dir.is_dir());
    assert_eq!(
        fs::read(extracted_dir.join("01.jpg")).unwrap(),
        b"page 1 bytes"
    );
    assert_eq!(
        fs::read(extracted_dir.join("02.png")).unwrap(),
        b"page 2 bytes"
    );
}

#[test]
fn test_convert_directory_to_cbz() {
    let tmp = tempdir().unwrap();
    let chapter_dir = tmp.path().join("my_comic");
    fs::create_dir_all(&chapter_dir).unwrap();
    fs::write(chapter_dir.join("01.jpg"), b"first image").unwrap();
    fs::write(chapter_dir.join("02.png"), b"second image").unwrap();

    // Convert directory into a cbz archive
    run_convert(&[chapter_dir], "cbz").unwrap();

    let output_cbz = tmp.path().join("my_comic.cbz");
    assert!(output_cbz.is_file());

    let verify_dir = tmp.path().join("verified");
    extract_archive(ArchiveKind::Cbz, &output_cbz, &verify_dir).unwrap();
    assert_eq!(fs::read(verify_dir.join("01.jpg")).unwrap(), b"first image");
    assert_eq!(
        fs::read(verify_dir.join("02.png")).unwrap(),
        b"second image"
    );
}

#[test]
fn test_convert_directory_to_cbr() {
    let tmp = tempdir().unwrap();
    let chapter_dir = tmp.path().join("my_comic_cbr");
    fs::create_dir_all(&chapter_dir).unwrap();
    fs::write(chapter_dir.join("01.jpg"), b"rar packed image").unwrap();

    // Convert directory into a cbr archive
    run_convert(&[chapter_dir], "cbr").unwrap();

    let output_cbr = tmp.path().join("my_comic_cbr.cbr");
    assert!(output_cbr.is_file());

    let verify_dir = tmp.path().join("verified_cbr");
    extract_archive(ArchiveKind::Cbr, &output_cbr, &verify_dir).unwrap();
    assert_eq!(
        fs::read(verify_dir.join("01.jpg")).unwrap(),
        b"rar packed image"
    );
}

#[test]
fn test_convert_directory_to_dir_skips() {
    let tmp = tempdir().unwrap();
    let chapter_dir = tmp.path().join("already_dir");
    fs::create_dir_all(&chapter_dir).unwrap();
    fs::write(chapter_dir.join("01.jpg"), b"content").unwrap();

    // Converting a directory to dir should skip gracefully
    assert!(run_convert(&[chapter_dir], "dir").is_ok());
}

#[test]
fn test_convert_container_directory_of_archives_to_dir() {
    let tmp = tempdir().unwrap();
    let container = tmp.path().join("comics_container");
    fs::create_dir_all(&container).unwrap();

    let raw = tmp.path().join("raw");
    fs::create_dir_all(&raw).unwrap();
    fs::write(raw.join("page.png"), b"container page").unwrap();

    let arc1 = container.join("vol1.cbz");
    let arc2 = container.join("vol2.cbt");
    compress_archive(ArchiveKind::Cbz, &raw, &arc1).unwrap();
    compress_archive(ArchiveKind::Cbt, &raw, &arc2).unwrap();

    // Convert container folder to dir -> unpacks both archives inside
    run_convert(&[container], "dir").unwrap();

    assert!(tmp.path().join("comics_container/vol1").is_dir());
    assert!(tmp.path().join("comics_container/vol2").is_dir());
    assert_eq!(
        fs::read(tmp.path().join("comics_container/vol1/page.png")).unwrap(),
        b"container page"
    );
    assert_eq!(
        fs::read(tmp.path().join("comics_container/vol2/page.png")).unwrap(),
        b"container page"
    );
}

#[test]
fn test_convert_container_directory_of_chapter_folders_to_cbz() {
    let tmp = tempdir().unwrap();
    let series = tmp.path().join("manga_series");
    let ch1 = series.join("ch01");
    let ch2 = series.join("ch02");
    fs::create_dir_all(&ch1).unwrap();
    fs::create_dir_all(&ch2).unwrap();
    fs::write(ch1.join("01.jpg"), b"ch1 page").unwrap();
    fs::write(ch2.join("01.jpg"), b"ch2 page").unwrap();

    // Convert container of chapter folders to cbz
    run_convert(&[series], "cbz").unwrap();

    let cbz1 = tmp.path().join("manga_series/ch01.cbz");
    let cbz2 = tmp.path().join("manga_series/ch02.cbz");
    assert!(cbz1.is_file());
    assert!(cbz2.is_file());

    let test_extract = tmp.path().join("test_extract");
    extract_archive(ArchiveKind::Cbz, &cbz1, &test_extract).unwrap();
    assert_eq!(fs::read(test_extract.join("01.jpg")).unwrap(), b"ch1 page");
}

#[test]
fn test_roundtrip_archive_to_directory_to_archive() {
    let tmp = tempdir().unwrap();
    let raw = tmp.path().join("raw");
    fs::create_dir_all(&raw).unwrap();
    fs::write(raw.join("01.jpg"), b"roundtrip 1").unwrap();
    fs::write(raw.join("02.jpg"), b"roundtrip 2").unwrap();

    let initial_cbz = tmp.path().join("roundtrip.cbz");
    compress_archive(ArchiveKind::Cbz, &raw, &initial_cbz).unwrap();

    // 1. Unpack archive to directory
    run_convert(&[initial_cbz], "dir").unwrap();
    let unpacked_dir = tmp.path().join("roundtrip");
    assert!(unpacked_dir.is_dir());

    // Rename to avoid in-place overwrite conflict during test
    let re_dir = tmp.path().join("repacked");
    fs::rename(unpacked_dir, &re_dir).unwrap();

    // 2. Repack directory into CBZ archive
    run_convert(&[re_dir], "cbz").unwrap();
    let repacked_cbz = tmp.path().join("repacked.cbz");
    assert!(repacked_cbz.is_file());

    // Verify repacked content
    let final_verify = tmp.path().join("final_verify");
    extract_archive(ArchiveKind::Cbz, &repacked_cbz, &final_verify).unwrap();
    assert_eq!(
        fs::read(final_verify.join("01.jpg")).unwrap(),
        b"roundtrip 1"
    );
    assert_eq!(
        fs::read(final_verify.join("02.jpg")).unwrap(),
        b"roundtrip 2"
    );
}

#[test]
fn test_convert_archive_with_root_dir_avoids_extra_wrapping_dir() {
    let tmp = tempdir().unwrap();
    let cbz_path = tmp.path().join("Issue_01.cbz");
    let mut writer = ArchiveWriter::new(ArchiveKind::Cbz, &cbz_path).unwrap();

    // Archive entries already prefixed with "Issue_01/" and nested "Issue_01/extras/"
    writer
        .add_entry("Issue_01/01.jpg", false, b"page one")
        .unwrap();
    writer
        .add_entry("Issue_01/02.jpg", false, b"page two")
        .unwrap();
    writer
        .add_entry("Issue_01/extras/bonus.jpg", false, b"bonus art")
        .unwrap();
    writer.finish().unwrap();

    // Unpack archive into directory using run_convert
    run_convert(&[cbz_path], "dir").unwrap();

    let extracted_dir = tmp.path().join("Issue_01");
    assert!(extracted_dir.is_dir());

    // Verify no extra "Issue_01/Issue_01" directory was created on top
    assert!(!extracted_dir.join("Issue_01").exists());

    // Verify directory contents stay the same as before extraction (not flattened)
    assert_eq!(fs::read(extracted_dir.join("01.jpg")).unwrap(), b"page one");
    assert_eq!(fs::read(extracted_dir.join("02.jpg")).unwrap(), b"page two");
    assert_eq!(
        fs::read(extracted_dir.join("extras").join("bonus.jpg")).unwrap(),
        b"bonus art"
    );
}

#[test]
fn test_convert_archive_with_mismatched_root_dir_avoids_extra_dir() {
    let tmp = tempdir().unwrap();
    let cbz_path = tmp.path().join("chapter_01.cbz");
    let mut writer = ArchiveWriter::new(ArchiveKind::Cbz, &cbz_path).unwrap();

    // Internal root is "Chapter 01" while archive is "chapter_01.cbz"
    writer
        .add_entry("Chapter 01/01.jpg", false, b"first page")
        .unwrap();
    writer
        .add_entry("Chapter 01/sub/02.jpg", false, b"second page")
        .unwrap();
    writer.finish().unwrap();

    run_convert(&[cbz_path], "dir").unwrap();

    let extracted_dir = tmp.path().join("chapter_01");
    assert!(extracted_dir.is_dir());

    // Verify no extra "Chapter 01" directory inside chapter_01
    assert!(!extracted_dir.join("Chapter 01").exists());
    assert!(!extracted_dir.join("chapter_01").exists());

    // Verify contents are directly in chapter_01 and subdirectories are not flattened
    assert_eq!(
        fs::read(extracted_dir.join("01.jpg")).unwrap(),
        b"first page"
    );
    assert_eq!(
        fs::read(extracted_dir.join("sub").join("02.jpg")).unwrap(),
        b"second page"
    );
}

#[test]
fn test_convert_misnamed_archive_detected_by_magic_bytes() {
    let tmp = tempdir().unwrap();

    // Create a RAR (CBR) archive but save it with a .cbz extension
    let misnamed_cbr = tmp.path().join("misnamed.cbz");
    {
        let mut writer = ArchiveWriter::new(ArchiveKind::Cbr, &misnamed_cbr).unwrap();
        writer.add_entry("01.jpg", false, b"image content").unwrap();
        writer.finish().unwrap();
    }

    // Verify detect_archive_kind identifies it as Cbr based on magic bytes, not Cbz from extension
    assert_eq!(detect_archive_kind(&misnamed_cbr), Some(ArchiveKind::Cbr));

    // Converting to cb7 should succeed because magic bytes allow reading it as RAR
    run_convert(&[misnamed_cbr], "cb7").unwrap();

    let output_cb7 = tmp.path().join("misnamed.cb7");
    assert!(output_cb7.is_file());
    assert_eq!(detect_archive_kind(&output_cb7), Some(ArchiveKind::Cb7));

    let extract_dir = tmp.path().join("extracted_cb7");
    extract_archive(ArchiveKind::Cb7, &output_cb7, &extract_dir).unwrap();
    assert_eq!(
        fs::read(extract_dir.join("01.jpg")).unwrap(),
        b"image content"
    );
}

// ===================================================================
// Clamp Command Tests & Validations
// ===================================================================

#[test]
fn test_clamp_validation_same_dir() {
    let tmp = tempdir().unwrap();
    let res = run_clamp(tmp.path(), tmp.path(), 5_000_000, Approach::Split, 1);
    assert!(res.is_err());
    assert!(res
        .unwrap_err()
        .to_string()
        .contains("Cannot save into the same directory"));
}

#[test]
fn test_clamp_validation_threshold_too_low() {
    let tmp = tempdir().unwrap();
    let out = tmp.path().join("out");

    // Split requires > 500,000
    let res_split = run_clamp(tmp.path(), &out, 500_000, Approach::Split, 1);
    assert!(res_split.is_err());
    assert!(res_split
        .unwrap_err()
        .to_string()
        .contains("must be > 500,000 pixels"));

    // Resize requires > 500,000
    let res_resize = run_clamp(tmp.path(), &out, 400_000, Approach::Resize, 1);
    assert!(res_resize.is_err());
    assert!(res_resize
        .unwrap_err()
        .to_string()
        .contains("must be > 500,000 pixels"));

    // MaxWidth requires > 400
    let res_width = run_clamp(tmp.path(), &out, 400, Approach::MaxWidth, 1);
    assert!(res_width.is_err());
    assert!(res_width
        .unwrap_err()
        .to_string()
        .contains("must be > 400 pixels"));
}

#[test]
fn test_clamp_under_threshold_direct_extraction() {
    let tmp = tempdir().unwrap();
    let input_dir = tmp.path().join("input");
    let output_dir = tmp.path().join("output");
    fs::create_dir_all(&input_dir).unwrap();

    let raw = tmp.path().join("raw");
    fs::create_dir_all(&raw).unwrap();
    // 500 x 500 = 250,000 pixels (far below 1,000,000)
    let img = DynamicImage::ImageRgb8(RgbImage::new(500, 500));
    img.save(raw.join("original.png")).unwrap();

    let cbz = input_dir.join("chap1.cbz");
    compress_archive(ArchiveKind::Cbz, &raw, &cbz).unwrap();

    // Run clamp with threshold 1,000,000
    run_clamp(&input_dir, &output_dir, 1_000_000, Approach::Split, 1).unwrap();

    let out_chap = output_dir.join("chap1");
    assert!(out_chap.exists());
    // Since it's already under threshold, original archive was extracted directly
    assert!(out_chap.join("original.png").exists());
}

#[test]
fn test_clamp_resize_approach() {
    let tmp = tempdir().unwrap();
    let input_dir = tmp.path().join("input");
    let output_dir = tmp.path().join("output");
    fs::create_dir_all(&input_dir).unwrap();

    let raw = tmp.path().join("raw");
    fs::create_dir_all(&raw).unwrap();
    // 2000 x 2000 = 4,000,000 pixels
    let img = DynamicImage::ImageRgb8(RgbImage::new(2000, 2000));
    img.save(raw.join("big.png")).unwrap();

    let cbz = input_dir.join("chap_resize.cbz");
    compress_archive(ArchiveKind::Cbz, &raw, &cbz).unwrap();

    // Resize to max 1,000,000 pixels
    run_clamp(&input_dir, &output_dir, 1_000_000, Approach::Resize, 1).unwrap();

    let out_chap = output_dir.join("chap_resize");
    assert!(out_chap.exists());
    let webp_file = out_chap.join("001.webp");
    assert!(webp_file.exists());

    let decoded = image::open(&webp_file).unwrap();
    let (w, h) = decoded.dimensions();
    assert!((w as u64) * (h as u64) <= 1_000_000);
}

#[test]
fn test_clamp_max_width_approach() {
    let tmp = tempdir().unwrap();
    let input_dir = tmp.path().join("input");
    let output_dir = tmp.path().join("output");
    fs::create_dir_all(&input_dir).unwrap();

    let raw = tmp.path().join("raw");
    fs::create_dir_all(&raw).unwrap();
    // 1600 x 900
    let img = DynamicImage::ImageRgb8(RgbImage::new(1600, 900));
    img.save(raw.join("wide.png")).unwrap();

    let cbz = input_dir.join("chap_width.cbz");
    compress_archive(ArchiveKind::Cbz, &raw, &cbz).unwrap();

    // Clamp width to max 800 px
    run_clamp(&input_dir, &output_dir, 800, Approach::MaxWidth, 1).unwrap();

    let out_chap = output_dir.join("chap_width");
    assert!(out_chap.exists());
    let webp_file = out_chap.join("001.webp");
    assert!(webp_file.exists());

    let decoded = image::open(&webp_file).unwrap();
    let (w, h) = decoded.dimensions();
    assert_eq!(w, 800);
    assert_eq!(h, 450);
}

#[test]
fn test_clamp_single_file_input() {
    let tmp = tempdir().unwrap();
    let raw = tmp.path().join("raw");
    fs::create_dir_all(&raw).unwrap();
    // 2000 x 3000 = 6,000,000 pixels
    let img = DynamicImage::ImageRgb8(RgbImage::new(2000, 3000));
    img.save(raw.join("page.png")).unwrap();

    let single_archive = tmp.path().join("standalone.cbz");
    compress_archive(ArchiveKind::Cbz, &raw, &single_archive).unwrap();

    let output_dir = tmp.path().join("out_single");
    // Pass the file directly as input_path
    run_clamp(&single_archive, &output_dir, 5_000_000, Approach::Split, 1).unwrap();

    let out_chap = output_dir.join("standalone");
    assert!(out_chap.exists());
    assert!(out_chap.join("001.webp").exists());
    assert!(out_chap.join("002.webp").exists());
}

#[test]
fn test_clamp_missing_input_path_errors() {
    let tmp = tempdir().unwrap();
    let missing = tmp.path().join("does_not_exist");
    let out = tmp.path().join("out");

    let res = run_clamp(&missing, &out, 5_000_000, Approach::Split, 1);
    assert!(res.is_err());
    assert!(res.unwrap_err().to_string().contains("does not exist"));
}

#[test]
fn test_clamp_unsupported_file_input_errors() {
    let tmp = tempdir().unwrap();
    let file = tmp.path().join("notes.txt");
    fs::write(&file, b"just some text, not an archive").unwrap();
    let out = tmp.path().join("out");

    let res = run_clamp(&file, &out, 5_000_000, Approach::Split, 1);
    assert!(res.is_err());
    assert!(res
        .unwrap_err()
        .to_string()
        .contains("Unsupported file type"));
}

#[test]
fn test_clamp_empty_input_directory_is_noop() {
    let tmp = tempdir().unwrap();
    let input = tmp.path().join("empty");
    fs::create_dir_all(&input).unwrap();
    let out = tmp.path().join("out");

    run_clamp(&input, &out, 5_000_000, Approach::Split, 1).unwrap();

    assert!(out.exists());
    assert_eq!(fs::read_dir(&out).unwrap().count(), 0);
}

#[test]
fn test_clamp_threshold_one_above_floor_is_accepted() {
    let tmp = tempdir().unwrap();
    let input = tmp.path().join("input");
    fs::create_dir_all(&input).unwrap();

    // Each value is exactly one above the approach's floor, so validation passes.
    run_clamp(
        &input,
        &tmp.path().join("out_split"),
        500_001,
        Approach::Split,
        1,
    )
    .unwrap();
    run_clamp(
        &input,
        &tmp.path().join("out_resize"),
        500_001,
        Approach::Resize,
        1,
    )
    .unwrap();
    run_clamp(
        &input,
        &tmp.path().join("out_width"),
        401,
        Approach::MaxWidth,
        1,
    )
    .unwrap();
}

#[test]
fn test_clamp_threshold_equality_still_reencodes() {
    let tmp = tempdir().unwrap();
    let input = tmp.path().join("input");
    fs::create_dir_all(&input).unwrap();

    let raw = tmp.path().join("raw");
    fs::create_dir_all(&raw).unwrap();
    // Exactly 1,000,000 pixels: equal to the threshold used below.
    let img = DynamicImage::ImageRgb8(RgbImage::new(1000, 1000));
    img.save(raw.join("page.png")).unwrap();
    compress_archive(ArchiveKind::Cbz, &raw, input.join("chap.cbz")).unwrap();

    // "Within threshold" is a strict `<`, so an exact match is still re-encoded.
    run_clamp(
        &input,
        &tmp.path().join("out"),
        1_000_000,
        Approach::Split,
        1,
    )
    .unwrap();

    let out_chap = tmp.path().join("out").join("chap");
    assert!(out_chap.join("001.webp").exists());
    assert!(out_chap.join("002.webp").exists());
    // The single 1000x1000 page became two 1000x500 halves.
    let first = image::open(out_chap.join("001.webp")).unwrap();
    assert_eq!(first.dimensions(), (1000, 500));
}

#[test]
fn test_clamp_orders_pages_naturally() {
    let tmp = tempdir().unwrap();
    let input = tmp.path().join("input");
    fs::create_dir_all(&input).unwrap();

    let raw = tmp.path().join("raw");
    fs::create_dir_all(&raw).unwrap();
    // Solid colours make page order observable in the numbered output.
    DynamicImage::ImageRgb8(RgbImage::from_pixel(1000, 1000, Rgb([255, 0, 0])))
        .save(raw.join("page_2.png"))
        .unwrap();
    DynamicImage::ImageRgb8(RgbImage::from_pixel(1000, 1000, Rgb([0, 0, 255])))
        .save(raw.join("page_10.png"))
        .unwrap();
    compress_archive(ArchiveKind::Cbz, &raw, input.join("chap.cbz")).unwrap();

    // Resize keeps a 1:1 page-to-file mapping, so colours map to indices directly.
    run_clamp(
        &input,
        &tmp.path().join("out"),
        500_001,
        Approach::Resize,
        1,
    )
    .unwrap();

    let out_chap = tmp.path().join("out").join("chap");
    let first = image::open(out_chap.join("001.webp")).unwrap().to_rgb8();
    let second = image::open(out_chap.join("002.webp")).unwrap().to_rgb8();

    // Natural order puts page_2 before page_10.
    let first_px = first.get_pixel(0, 0).0;
    assert!(
        first_px[0] > 200 && first_px[2] < 60,
        "001.webp should be the red page_2, got {first_px:?}"
    );
    let second_px = second.get_pixel(0, 0).0;
    assert!(
        second_px[2] > 200 && second_px[0] < 60,
        "002.webp should be the blue page_10, got {second_px:?}"
    );
}

#[test]
fn test_clamp_directory_source_uses_directory_name() {
    let tmp = tempdir().unwrap();
    let input = tmp.path().join("library");
    let chapter = input.join("chapter_a");
    fs::create_dir_all(&chapter).unwrap();
    let img = DynamicImage::ImageRgb8(RgbImage::new(1000, 1000));
    img.save(chapter.join("page.png")).unwrap();

    run_clamp(&input, &tmp.path().join("out"), 500_001, Approach::Split, 1).unwrap();

    let out_chap = tmp.path().join("out").join("chapter_a");
    assert!(out_chap.join("001.webp").exists());
    assert!(out_chap.join("002.webp").exists());
}

#[test]
fn test_clamp_under_threshold_directory_source_copies_files() {
    let tmp = tempdir().unwrap();
    let input = tmp.path().join("library");
    let chapter = input.join("small_chapter");
    fs::create_dir_all(&chapter).unwrap();
    // 500 x 500 = 250,000 pixels, far below the threshold.
    let img = DynamicImage::ImageRgb8(RgbImage::new(500, 500));
    img.save(chapter.join("original.png")).unwrap();

    run_clamp(
        &input,
        &tmp.path().join("out"),
        1_000_000,
        Approach::Split,
        1,
    )
    .unwrap();

    // Under the threshold the folder is copied verbatim, keeping the original name.
    assert!(tmp
        .path()
        .join("out")
        .join("small_chapter")
        .join("original.png")
        .exists());
}

#[test]
fn test_clamp_skips_output_directory_inside_input() {
    let tmp = tempdir().unwrap();
    let input = tmp.path().join("comics");
    fs::create_dir_all(&input).unwrap();

    let raw = tmp.path().join("raw");
    fs::create_dir_all(&raw).unwrap();
    let img = DynamicImage::ImageRgb8(RgbImage::new(1000, 1000));
    img.save(raw.join("page.png")).unwrap();
    compress_archive(ArchiveKind::Cbz, &raw, input.join("chap.cbz")).unwrap();

    // The output directory lives inside the input directory.
    let out = input.join("Results");
    run_clamp(&input, &out, 500_001, Approach::Resize, 1).unwrap();

    assert!(out.join("chap").join("001.webp").exists());
    // The output directory must not be swept up as just another chapter.
    assert!(!out.join("Results").exists());
}

#[test]
fn test_clamp_removes_stale_output_on_rerun() {
    let tmp = tempdir().unwrap();
    let input = tmp.path().join("input");
    fs::create_dir_all(&input).unwrap();

    let raw = tmp.path().join("raw");
    fs::create_dir_all(&raw).unwrap();
    let img = DynamicImage::ImageRgb8(RgbImage::new(1000, 1000));
    img.save(raw.join("page.png")).unwrap();
    compress_archive(ArchiveKind::Cbz, &raw, input.join("chap.cbz")).unwrap();

    let out = tmp.path().join("out");
    let stale = out.join("chap").join("stale.webp");
    fs::create_dir_all(stale.parent().unwrap()).unwrap();
    fs::write(&stale, b"left over from a previous run").unwrap();

    run_clamp(&input, &out, 500_001, Approach::Resize, 1).unwrap();

    assert!(out.join("chap").join("001.webp").exists());
    // The existing chapter directory is replaced, dropping the stale file.
    assert!(!stale.exists());
}

#[test]
fn test_clamp_processes_multiple_chapters_in_parallel() {
    let tmp = tempdir().unwrap();
    let input = tmp.path().join("input");
    let folder_chapter = input.join("chapter_c");
    fs::create_dir_all(&folder_chapter).unwrap();

    let raw = tmp.path().join("raw");
    fs::create_dir_all(&raw).unwrap();
    let img = DynamicImage::ImageRgb8(RgbImage::new(1000, 1000));
    img.save(raw.join("page.png")).unwrap();
    img.save(folder_chapter.join("page.png")).unwrap();

    compress_archive(ArchiveKind::Cbz, &raw, input.join("chapter_a.cbz")).unwrap();
    compress_archive(ArchiveKind::Cbt, &raw, input.join("chapter_b.cbt")).unwrap();

    let out = tmp.path().join("out");
    run_clamp(&input, &out, 500_001, Approach::Resize, 4).unwrap();

    for name in ["chapter_a", "chapter_b", "chapter_c"] {
        assert!(
            out.join(name).join("001.webp").exists(),
            "missing clamped output for {name}"
        );
    }
}

#[test]
fn test_clamp_directory_chapter_keeps_dotted_name() {
    let tmp = tempdir().unwrap();
    let input = tmp.path().join("library");
    let chapter = input.join("chapter.02");
    fs::create_dir_all(&chapter).unwrap();
    let img = DynamicImage::ImageRgb8(RgbImage::new(1000, 1000));
    img.save(chapter.join("page.png")).unwrap();

    run_clamp(&input, &tmp.path().join("out"), 500_001, Approach::Split, 1).unwrap();

    // A directory source keeps its full name, including the dot, rather than its stem.
    assert!(tmp
        .path()
        .join("out")
        .join("chapter.02")
        .join("001.webp")
        .exists());
}

#[test]
fn test_clamp_validation_error_messages_are_specific() {
    let tmp = tempdir().unwrap();
    let input = tmp.path().join("input");
    fs::create_dir_all(&input).unwrap();

    let message = |threshold, approach, name: &str| {
        run_clamp(&input, &tmp.path().join(name), threshold, approach, 1)
            .unwrap_err()
            .to_string()
    };

    assert_eq!(
        message(500_000, Approach::Split, "o1"),
        "For 'split' or 'resize' approach, size_threshold must be > 500,000 pixels"
    );
    assert_eq!(
        message(400_000, Approach::Resize, "o2"),
        "For 'split' or 'resize' approach, size_threshold must be > 500,000 pixels"
    );
    assert_eq!(
        message(400, Approach::MaxWidth, "o3"),
        "For 'max-width' approach, size_threshold must be > 400 pixels"
    );
}

#[test]
fn test_remove_dir_all_force_is_idempotent() {
    let tmp = tempdir().unwrap();

    // A path that does not exist is not an error.
    let missing = tmp.path().join("missing");
    assert!(remove_dir_all_force(&missing).is_ok());

    // A populated directory (including nested content) is removed recursively.
    let populated = tmp.path().join("populated");
    fs::create_dir_all(populated.join("nested")).unwrap();
    fs::write(populated.join("nested").join("file.txt"), b"data").unwrap();
    remove_dir_all_force(&populated).unwrap();
    assert!(!populated.exists());
}

// ===================================================================
// Cross-Platform Path Handling Tests
// ===================================================================

#[test]
fn test_normalize_archive_path() {
    // Unix forward slashes
    assert_eq!(normalize_archive_path("pages/001.png"), "pages/001.png");
    assert_eq!(normalize_archive_path("a/b/c/d.jpg"), "a/b/c/d.jpg");

    // Windows backslashes
    assert_eq!(normalize_archive_path("pages\\001.png"), "pages/001.png");
    assert_eq!(normalize_archive_path("a\\b\\c\\d.jpg"), "a/b/c/d.jpg");

    // Mixed slashes
    assert_eq!(normalize_archive_path("a/b\\c/d.jpg"), "a/b/c/d.jpg");

    // Leading and trailing slashes
    assert_eq!(normalize_archive_path("/pages/001.png/"), "pages/001.png");
    assert_eq!(
        normalize_archive_path("\\pages\\001.png\\"),
        "pages/001.png"
    );
    assert_eq!(normalize_archive_path("///a//b///"), "a/b");

    // Redundant current directory dots
    assert_eq!(normalize_archive_path("./pages/./001.png"), "pages/001.png");
    assert_eq!(
        normalize_archive_path(".\\pages\\.\\001.png"),
        "pages/001.png"
    );

    // Directory traversal segments
    assert_eq!(normalize_archive_path("../../etc/passwd"), "etc/passwd");
    assert_eq!(normalize_archive_path("..\\..\\secret.png"), "secret.png");
    assert_eq!(normalize_archive_path("a/../b/c.png"), "a/b/c.png");

    // Windows drive prefixes
    assert_eq!(
        normalize_archive_path("C:\\comics\\001.png"),
        "comics/001.png"
    );
    assert_eq!(
        normalize_archive_path("d:/comics/001.png"),
        "comics/001.png"
    );

    // Empty or separator-only paths
    assert_eq!(normalize_archive_path(""), "");
    assert_eq!(normalize_archive_path("/"), "");
    assert_eq!(normalize_archive_path("\\"), "");
    assert_eq!(normalize_archive_path("./"), "");
    assert_eq!(normalize_archive_path("."), "");
}

#[test]
fn test_safe_join() {
    let tmp = tempdir().unwrap();
    let base = tmp.path();

    // Standard relative paths with Unix slashes
    let joined_unix = safe_join(base, "chapter1/pages/001.png");
    assert_eq!(
        joined_unix,
        base.join("chapter1").join("pages").join("001.png")
    );

    // Standard relative paths with Windows backslashes
    let joined_win = safe_join(base, "chapter1\\pages\\001.png");
    assert_eq!(
        joined_win,
        base.join("chapter1").join("pages").join("001.png")
    );

    // Traversal attempts must not escape base
    let joined_escape = safe_join(base, "../../../system32/cmd.exe");
    assert_eq!(joined_escape, base.join("system32").join("cmd.exe"));

    // Absolute Unix root paths must not escape base
    let joined_abs = safe_join(base, "/etc/passwd");
    assert_eq!(joined_abs, base.join("etc").join("passwd"));

    // Windows drive paths must not escape base
    let joined_drive = safe_join(base, "C:\\Windows\\win.ini");
    assert_eq!(joined_drive, base.join("Windows").join("win.ini"));
}

#[test]
fn test_cross_platform_nested_directory_extraction() {
    let tmp = tempdir().unwrap();
    let cbz_path = tmp.path().join("nested.cbz");

    // Create CBZ with Windows-style backslash entry paths
    let mut writer = ArchiveWriter::new(ArchiveKind::Cbz, &cbz_path).unwrap();
    let dummy_img = DynamicImage::ImageRgb8(RgbImage::new(10, 10));
    let mut img_bytes = Vec::new();
    dummy_img
        .write_to(
            &mut std::io::Cursor::new(&mut img_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();

    writer
        .add_entry("vol1\\ch1\\p01.png", false, &img_bytes)
        .unwrap();
    writer
        .add_entry("vol1/ch2/p01.png", false, &img_bytes)
        .unwrap();
    writer.finish().unwrap();

    // Verify images retrieved regardless of separator used
    let images = get_images_from_source(ArchiveKind::Cbz, &cbz_path).unwrap();
    assert_eq!(images.len(), 2);
    assert_eq!(images[0].0, "p01.png");
    assert_eq!(images[1].0, "p01.png");

    // Extract to disk and verify nested directory structure created natively
    let extract_dir = tmp.path().join("extracted_nested");
    extract_archive(ArchiveKind::Cbz, &cbz_path, &extract_dir).unwrap();

    assert!(extract_dir
        .join("vol1")
        .join("ch1")
        .join("p01.png")
        .exists());
    assert!(extract_dir
        .join("vol1")
        .join("ch2")
        .join("p01.png")
        .exists());
}

#[test]
fn test_is_image_file_with_mixed_separators() {
    assert!(is_image_file(Path::new("normal.png")));
    assert!(is_image_file(Path::new("folder/image.jpg")));
    assert!(is_image_file(Path::new("folder\\image.webp")));
    assert!(is_image_file(Path::new("C:\\comics\\cover.jpeg")));
    assert!(is_image_file(Path::new("/var/comics/page.PNG")));
    assert!(!is_image_file(Path::new("folder\\not_image.txt")));
    assert!(!is_image_file(Path::new("archive.cbz")));
}
