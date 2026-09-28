//! Tests for the `comic-book ebook` CLI surface: parsing, option resolution and the
//! device profile tables.

use anyhow::{anyhow, bail, Result};
use clap::error::ErrorKind;
use clap::{CommandFactory, Parser};
use comic_book::cli::{Cli, Commands};
use comic_book::ebook::options::{
    BatchSplit, BorderColor, Cropping, DocType, Format, InterPanelCrop, MetadataTitle, Options,
    Splitter,
};
use comic_book::ebook::profiles::{DeviceKind, Profile, ALL_PROFILES, PROFILE_TABLE};

/// Parse `comic-book ebook <args>` and resolve it.
fn resolve(args: &[&str]) -> Result<Options> {
    let args = ebook_args(args)?;
    Options::resolve(&args)
}

/// Parse `comic-book ebook <args>`, expecting option resolution to fail.
fn resolve_err(args: &[&str]) -> Result<anyhow::Error> {
    let args = ebook_args(args)?;
    match Options::resolve(&args) {
        Ok(_) => bail!("option resolution should fail"),
        Err(error) => Ok(error),
    }
}

fn ebook_args(args: &[&str]) -> Result<comic_book::ebook::EbookArgs> {
    let mut full = vec!["comic-book", "ebook"];
    full.extend_from_slice(args);
    let cli = Cli::try_parse_from(full)?;
    match cli.command {
        Commands::Ebook(args) => Ok(args),
        other => bail!("expected the ebook subcommand, got {other:?}"),
    }
}

#[test]
fn cli_definition_is_valid() {
    Cli::command().debug_assert();
}

#[test]
fn help_and_version_are_available_on_the_subcommand() -> Result<()> {
    let Err(help) = Cli::try_parse_from(["comic-book", "ebook", "--help"]) else {
        bail!("--help should be handled by clap");
    };
    assert_eq!(help.kind(), ErrorKind::DisplayHelp);

    let Err(version) = Cli::try_parse_from(["comic-book", "ebook", "--version"]) else {
        bail!("--version should be handled by clap");
    };
    assert_eq!(version.kind(), ErrorKind::DisplayVersion);
    Ok(())
}

#[test]
fn help_expands_every_profile_acronym() -> Result<()> {
    let mut command = Cli::command();
    let ebook = command
        .find_subcommand_mut("ebook")
        .ok_or_else(|| anyhow!("the ebook subcommand should exist"))?;
    let help = ebook.render_long_help().to_string();

    for entry in PROFILE_TABLE.iter() {
        assert!(
            help.contains(entry.code),
            "help should list the profile code {}",
            entry.code
        );
        assert!(
            help.contains(entry.label),
            "help should expand {} to its device name {}",
            entry.code,
            entry.label
        );
    }
    Ok(())
}

#[test]
fn every_documented_format_parses() -> Result<()> {
    for (value, expected) in [
        ("auto", Format::Auto),
        ("epub", Format::Epub),
        ("kepub", Format::Kepub),
        ("azw3", Format::Azw3),
        ("mobi", Format::Mobi),
        ("mobi+epub", Format::MobiEpub),
        ("cbz", Format::Cbz),
        ("pdf", Format::Pdf),
        ("kfx", Format::Kfx),
        ("epub-200mb", Format::Epub200mb),
        ("pdf-200mb", Format::Pdf200mb),
        ("mobi+epub-200mb", Format::MobiEpub200mb),
    ] {
        assert_eq!(
            ebook_args(&["book.cbz", "-f", value])?.output.format,
            expected
        );
    }

    // Unknown formats are rejected by clap.
    assert!(Cli::try_parse_from(["comic-book", "ebook", "book.cbz", "-f", "djvu"]).is_err());
    Ok(())
}

#[test]
fn every_profile_code_parses_and_is_case_insensitive() -> Result<()> {
    for profile in ALL_PROFILES {
        let parsed = ebook_args(&["book.cbz", "-p", profile.code()])?
            .device
            .profile;
        assert_eq!(parsed, profile);

        let lowercase = profile.code().to_lowercase();
        let lower = ebook_args(&["book.cbz", "-p", lowercase.as_str()])?
            .device
            .profile;
        assert_eq!(
            lower, profile,
            "profile {profile} should parse case-insensitively"
        );
    }

    assert!(Cli::try_parse_from(["comic-book", "ebook", "book.cbz", "-p", "NOPE"]).is_err());
    Ok(())
}

#[test]
fn profile_table_is_consistent_with_the_variant_list() {
    assert_eq!(ALL_PROFILES.len(), PROFILE_TABLE.len());
    for (idx, entry) in PROFILE_TABLE.iter().enumerate() {
        assert_eq!(entry.profile, ALL_PROFILES[idx]);
        assert_eq!(Profile::from_code(entry.code), Some(entry.profile));
    }

    // Codes are unique.
    let mut codes: Vec<&str> = PROFILE_TABLE.iter().map(|entry| entry.code).collect();
    codes.sort_unstable();
    let unique = codes.len();
    codes.dedup();
    assert_eq!(codes.len(), unique);

    // Spot-check a few rows against the profile table (docs/cli.md).
    assert_eq!(
        Profile::Kv.data(),
        comic_book::ebook::profiles::ProfileData {
            name: "Kindle Voyage".to_string(),
            width: 1072,
            height: 1448,
            palette: &comic_book::ebook::profiles::PALETTE16,
            gamma: 1.0,
        }
    );
    assert_eq!(Profile::KoE.device_kind(), DeviceKind::Kobo);
    assert_eq!(Profile::RmkPp.device_kind(), DeviceKind::Remarkable);
    assert_eq!(Profile::Other.device_kind(), DeviceKind::Other);
    assert_eq!(
        Profile::K1.data().palette,
        &comic_book::ebook::profiles::PALETTE4
    );
}

#[test]
fn processing_mode_enums_accept_names_and_numeric_aliases() -> Result<()> {
    // Every named value and its legacy numeric alias resolve to the same mode.
    for (values, expected) in [
        (["split", "0"], Splitter::Split),
        (["rotate", "1"], Splitter::Rotate),
        (["both", "2"], Splitter::Both),
    ] {
        for value in values {
            let options = resolve(&["book.cbz", "-p", "KV", "--splitter", value])?;
            assert_eq!(options.splitter, expected, "--splitter {value}");
        }
    }

    for (values, expected) in [
        (["off", "0"], Cropping::Off),
        (["margins", "1"], Cropping::Margins),
        (["pages", "2"], Cropping::PageNumbers),
    ] {
        for value in values {
            let options = resolve(&["book.cbz", "-p", "KV", "-c", value])?;
            assert_eq!(options.cropping, expected, "--cropping {value}");
        }
    }

    for (values, expected) in [
        (["off", "0"], InterPanelCrop::Off),
        (["horizontal", "1"], InterPanelCrop::Horizontal),
        (["both", "2"], InterPanelCrop::Both),
    ] {
        for value in values {
            let options = resolve(&["book.cbz", "-p", "KV", "--inter-panel-crop", value])?;
            assert_eq!(
                options.inter_panel_crop, expected,
                "--inter-panel-crop {value}"
            );
        }
    }

    for (values, expected) in [
        (["default", "0"], MetadataTitle::Default),
        (["combine", "1"], MetadataTitle::Combine),
        (["only", "2"], MetadataTitle::Only),
    ] {
        for value in values {
            let options = resolve(&["book.cbz", "-p", "KV", "--metadata-title", value])?;
            assert_eq!(options.metadata_title, expected, "--metadata-title {value}");
        }
    }

    // A Kobo EPUB is used so the MOBI-only "always split" rule does not mask the
    // requested mode.
    for (values, expected) in [
        (["none", "0"], BatchSplit::None),
        (["auto", "1"], BatchSplit::Auto),
        (["per-subdir", "2"], BatchSplit::PerSubdirectory),
    ] {
        for value in values {
            let options = resolve(&["book.cbz", "-p", "KoE", "-b", value])?;
            assert_eq!(options.batch_split, expected, "--batch-split {value}");
        }
    }

    Ok(())
}

#[test]
fn border_flags_map_to_colours_and_are_exclusive() -> Result<()> {
    assert_eq!(
        resolve(&["book.cbz", "-p", "KV", "--borders", "black"])?.borders_color,
        Some(BorderColor::Black)
    );
    assert_eq!(
        resolve(&["book.cbz", "-p", "KV", "--borders", "white"])?.borders_color,
        Some(BorderColor::White)
    );

    // The legacy spellings keep working as hidden aliases for the two colours.
    assert_eq!(
        resolve(&["book.cbz", "-p", "KV", "--black-borders"])?.borders_color,
        Some(BorderColor::Black)
    );
    assert_eq!(
        resolve(&["book.cbz", "-p", "KV", "--white-borders"])?.borders_color,
        Some(BorderColor::White)
    );

    // Requesting two colours at once is a clap error, whether new or legacy.
    assert!(Cli::try_parse_from([
        "comic-book",
        "ebook",
        "book.cbz",
        "--borders",
        "black",
        "--borders",
        "white"
    ])
    .is_err());
    assert!(Cli::try_parse_from([
        "comic-book",
        "ebook",
        "book.cbz",
        "--black-borders",
        "--white-borders"
    ])
    .is_err());
    assert!(Cli::try_parse_from([
        "comic-book",
        "ebook",
        "book.cbz",
        "--borders",
        "black",
        "--white-borders"
    ])
    .is_err());
    Ok(())
}

#[test]
fn defaults_resolve_to_kindle_mobi() -> Result<()> {
    let options = resolve(&["book.cbz"])?;
    assert_eq!(options.profile, Profile::Kv);
    assert!(options.is_kindle);
    assert!(!options.is_kobo);
    assert_eq!(options.format, Format::Mobi);
    assert_eq!(options.jpeg_quality, 85);
    assert_eq!(options.target_size, None);
    assert_eq!(
        options.batch_split,
        BatchSplit::Auto,
        "MOBI output always splits"
    );
    assert!(options.kindle_azw3);
    assert!(!options.kepub);
    Ok(())
}

#[test]
fn kobo_epub_resolves_to_kepub() -> Result<()> {
    let options = resolve(&["book.cbz", "-p", "KoE", "-f", "epub"])?;
    assert!(!options.is_kindle);
    assert!(options.is_kobo);
    assert_eq!(options.format, Format::Epub);
    assert!(options.kepub, "Kobo brand + EPUB defaults to .kepub.epub");
    assert!(!options.panel_view, "Kobo disables panel view");
    Ok(())
}

#[test]
fn kobo_epub_with_no_kepub_stays_plain_epub() -> Result<()> {
    let options = resolve(&["book.cbz", "-p", "KoE", "-f", "epub", "--no-kepub"])?;
    assert!(!options.kepub);
    Ok(())
}

#[test]
fn explicit_kepub_format_sets_the_flag() -> Result<()> {
    let options = resolve(&["book.cbz", "-f", "kepub"])?;
    assert_eq!(options.format, Format::Epub);
    assert!(options.kepub);
    Ok(())
}

#[test]
fn auto_resolves_by_profile() -> Result<()> {
    assert_eq!(resolve(&["book.cbz", "-p", "KDX"])?.format, Format::Cbz);
    assert_eq!(resolve(&["book.cbz", "-p", "KV"])?.format, Format::Mobi);
    assert_eq!(resolve(&["book.cbz", "-p", "KoE"])?.format, Format::Epub);
    assert_eq!(resolve(&["book.cbz", "-p", "Rmk2"])?.format, Format::Pdf);
    Ok(())
}

#[test]
fn remarkable_gets_a_default_target_size() -> Result<()> {
    let options = resolve(&["book.cbz", "-p", "Rmk2"])?;
    assert_eq!(options.target_size, Some(95));
    Ok(())
}

#[test]
fn mobi_output_is_rejected_for_non_kindle_profiles() -> Result<()> {
    let err = resolve_err(&["book.cbz", "-p", "KoE", "-f", "mobi"])?;
    assert!(err
        .to_string()
        .contains("not supported for non-Kindle profiles"));

    let err = resolve_err(&["book.cbz", "-p", "Rmk2", "-f", "azw3"])?;
    assert!(err
        .to_string()
        .contains("not supported for non-Kindle profiles"));
    Ok(())
}

#[test]
fn mobi_epub_keeps_the_intermediate_epub() -> Result<()> {
    let options = resolve(&["book.cbz", "-f", "mobi+epub"])?;
    assert_eq!(options.format, Format::Mobi);
    assert!(options.keep_epub);
    Ok(())
}

#[test]
fn two_hundred_megabyte_presets_expand() -> Result<()> {
    let epub = resolve(&["book.cbz", "-f", "epub-200mb"])?;
    assert_eq!(epub.format, Format::Epub);
    assert_eq!(epub.target_size, Some(195));
    assert_eq!(epub.batch_split, BatchSplit::Auto);

    let pdf = resolve(&["book.cbz", "-f", "pdf-200mb"])?;
    assert_eq!(pdf.format, Format::Pdf);
    assert_eq!(pdf.target_size, Some(195));

    let mobi = resolve(&["book.cbz", "-f", "mobi+epub-200mb"])?;
    assert_eq!(mobi.format, Format::Mobi);
    assert!(mobi.keep_epub);
    assert_eq!(mobi.target_size, Some(195));
    Ok(())
}

#[test]
fn kfx_is_an_epub_preset_with_disabled_panel_view() -> Result<()> {
    let options = resolve(&["book.cbz", "-f", "kfx"])?;
    assert_eq!(options.format, Format::Epub);
    assert!(options.kfx);
    assert_eq!(options.target_size, Some(195));
    assert!(!options.panel_view);
    Ok(())
}

#[test]
fn jpeg_quality_defaults_by_device_and_honours_overrides() -> Result<()> {
    assert_eq!(resolve(&["book.cbz", "-p", "KV"])?.jpeg_quality, 85);
    assert_eq!(resolve(&["book.cbz", "-p", "KS"])?.jpeg_quality, 90);
    assert_eq!(resolve(&["book.cbz", "-p", "KCS"])?.jpeg_quality, 90);
    assert_eq!(
        resolve(&["book.cbz", "-p", "KV", "--jpeg-quality", "70"])?.jpeg_quality,
        70
    );
    assert!(
        Cli::try_parse_from(["comic-book", "ebook", "book.cbz", "--jpeg-quality", "99"]).is_err()
    );
    Ok(())
}

#[test]
fn custom_geometry_replaces_the_profile() -> Result<()> {
    let options = resolve(&[
        "book.cbz",
        "--custom-width",
        "1000",
        "--custom-height",
        "1500",
    ])?;
    assert!(options.custom_profile);
    assert_eq!(options.profile_data.name, "Custom");
    assert_eq!(options.profile_data.width, 1000);
    assert_eq!(options.profile_data.height, 1500);
    assert_eq!(
        options.profile,
        Profile::Kv,
        "the named profile is retained"
    );
    Ok(())
}

#[test]
fn the_other_profile_needs_an_explicit_size() -> Result<()> {
    // `OTHER` has no screen geometry of its own (see docs/cli.md), so it is only
    // usable with a custom resolution.
    let err = resolve_err(&["book.cbz", "-p", "OTHER"])?;
    assert!(
        err.to_string().contains("has no screen size"),
        "unexpected error: {err}"
    );

    let options = resolve(&[
        "book.cbz",
        "-p",
        "OTHER",
        "--custom-width",
        "1200",
        "--custom-height",
        "1600",
    ])?;
    assert_eq!(options.profile_data.width, 1200);
    assert_eq!(options.profile_data.height, 1600);
    Ok(())
}

#[test]
fn webtoon_mandates_its_option_set() -> Result<()> {
    let options = resolve(&["book.cbz", "-w", "-m", "-u", "-q"])?;
    assert!(!options.panel_view);
    assert!(!options.right_to_left);
    assert!(!options.upscale);
    assert!(!options.hq);
    assert_eq!(
        options.borders_color,
        Some(comic_book::ebook::BorderColor::White)
    );
    Ok(())
}

#[test]
fn legacy_kindles_disable_panel_view_and_hq() -> Result<()> {
    for profile in ["K1", "K2", "K34", "KDX"] {
        let options = resolve(&["book.cbz", "-p", profile, "-q", "--two-panel"])?;
        assert!(!options.panel_view, "{profile} has no panel view");
        assert!(!options.hq, "{profile} has no HQ mode");
    }
    Ok(())
}

#[test]
fn panel_view_requires_hq_or_panel_flags() -> Result<()> {
    let plain = resolve(&["book.cbz", "-p", "KV"])?;
    assert!(!plain.panel_view, "panel view is off unless requested");

    let with_hq = resolve(&["book.cbz", "-p", "KV", "-q"])?;
    assert!(with_hq.panel_view);

    let with_two_panel = resolve(&["book.cbz", "-p", "KV", "-2"])?;
    assert!(with_two_panel.panel_view);
    Ok(())
}

#[test]
fn kdx_cbz_raises_the_height() -> Result<()> {
    let options = resolve(&["book.cbz", "-p", "KDX"])?;
    assert_eq!(options.format, Format::Cbz);
    assert_eq!(options.profile_data.height, 1200);
    Ok(())
}

#[test]
fn scribe_azw3_caps_the_width() -> Result<()> {
    // KS is already narrower than the cap; KS3 exceeds it.
    let scribe = resolve(&["book.cbz", "-p", "KS"])?;
    assert!(scribe.kindle_scribe_azw3);
    assert_eq!(scribe.profile_data.width, 1860);

    let scribe3 = resolve(&["book.cbz", "-p", "KS3"])?;
    assert!(scribe3.kindle_scribe_azw3);
    assert_eq!(scribe3.profile_data.width, 1920);
    Ok(())
}

#[test]
fn doc_type_defaults_to_none_and_parses() -> Result<()> {
    assert_eq!(resolve(&["book.cbz"])?.doc_type, DocType::None);
    assert_eq!(
        resolve(&["book.cbz", "--doc-type", "ebok"])?.doc_type,
        DocType::Ebok
    );
    assert_eq!(
        resolve(&["book.cbz", "--doc-type", "pdoc"])?.doc_type,
        DocType::Pdoc
    );
    assert!(Cli::try_parse_from(["comic-book", "ebook", "book.cbz", "--doc-type", "x"]).is_err());
    Ok(())
}

#[test]
fn mozjpeg_is_rejected_with_a_clear_message() -> Result<()> {
    let err = resolve_err(&["book.cbz", "--mozjpeg"])?;
    assert!(err.to_string().contains("--mozjpeg is not supported"));
    Ok(())
}

#[test]
fn ebook_is_registered_in_completions_with_its_input_flags() -> Result<()> {
    let command = Cli::command();
    let Some(ebook) = command
        .get_subcommands()
        .find(|sub| sub.get_name() == "ebook")
    else {
        bail!("the ebook subcommand is registered");
    };
    let flags: Vec<String> = ebook
        .get_arguments()
        .map(|arg| arg.get_id().to_string())
        .collect();
    for expected in ["legacy_extract", "pdf_width", "profile", "format"] {
        assert!(flags.contains(&expected.to_string()), "missing {expected}");
    }
    Ok(())
}
