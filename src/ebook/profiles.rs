//! Device profiles and their screen geometry (see docs/cli.md).
//!
//! The tables below are re-implemented from KCC's documented behaviour — no GPL
//! source is copied (see docs/dependencies.md). Each row pins the screen resolution,
//! quantisation palette and device family a profile belongs to.

use clap::builder::PossibleValue;
use clap::ValueEnum;
use std::fmt;

/// Four-level greyscale palette used by the first-generation Kindle.
pub const PALETTE4: [u8; 12] = [
    0x00, 0x00, 0x00, 0x55, 0x55, 0x55, 0xaa, 0xaa, 0xaa, 0xff, 0xff, 0xff,
];

/// Fifteen-level greyscale palette used by the Kindle 2.
pub const PALETTE15: [u8; 45] = [
    0x00, 0x00, 0x00, 0x11, 0x11, 0x11, 0x22, 0x22, 0x22, 0x33, 0x33, 0x33, 0x44, 0x44, 0x44, 0x55,
    0x55, 0x55, 0x66, 0x66, 0x66, 0x77, 0x77, 0x77, 0x88, 0x88, 0x88, 0x99, 0x99, 0x99, 0xaa, 0xaa,
    0xaa, 0xbb, 0xbb, 0xbb, 0xcc, 0xcc, 0xcc, 0xdd, 0xdd, 0xdd, 0xff, 0xff, 0xff,
];

/// Sixteen-level greyscale palette used by every modern greyscale device.
pub const PALETTE16: [u8; 48] = [
    0x00, 0x00, 0x00, 0x11, 0x11, 0x11, 0x22, 0x22, 0x22, 0x33, 0x33, 0x33, 0x44, 0x44, 0x44, 0x55,
    0x55, 0x55, 0x66, 0x66, 0x66, 0x77, 0x77, 0x77, 0x88, 0x88, 0x88, 0x99, 0x99, 0x99, 0xaa, 0xaa,
    0xaa, 0xbb, 0xbb, 0xbb, 0xcc, 0xcc, 0xcc, 0xdd, 0xdd, 0xdd, 0xee, 0xee, 0xee, 0xff, 0xff, 0xff,
];

/// Sentinel palette meaning "do not quantise to a fixed palette".
pub const PALETTE_NULL: [u8; 0] = [];

/// The device family a [`Profile`] belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeviceKind {
    Kindle,
    Kobo,
    Remarkable,
    Other,
}

/// A supported output device profile.
///
/// The variant set mirrors KCC's `Profiles` dictionary; the canonical string
/// form (e.g. `KV`, `KoE`, `RmkPP`) is the row `code` in [`PROFILE_TABLE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Profile {
    // Kindle
    K1,
    K2,
    Kdx,
    K34,
    K57,
    Kpw,
    Kv,
    Kpw34,
    K810,
    Ko,
    K11,
    Kpw5,
    Kpw6,
    Ks1860,
    Ks1920,
    Ks1240,
    Ks1324,
    Ks,
    Kcs,
    Ks3,
    Kscs,
    // Kobo
    KoMt,
    KoG,
    KoGhd,
    KoA,
    KoAhd,
    KoAh2o,
    KoAo,
    KoN,
    KoC,
    KoCc,
    KoL,
    KoLc,
    KoF,
    KoS,
    KoE,
    // reMarkable
    Rmk1,
    Rmk2,
    RmkPp,
    RmkPpMove,
    // Generic
    Other,
}

/// Every profile, in [`PROFILE_TABLE`] order.
///
/// Derived from the table's own `profile` tags, so there is no second hand-kept list
/// of variants to drift out of sync; it drives the `--profile` value list.
pub static ALL_PROFILES: [Profile; PROFILE_ROWS.len()] = profile_variants(&PROFILE_ROWS);

/// The profile tag of each row, in row order (the seed is overwritten in full).
const fn profile_variants<const N: usize>(rows: &[ProfileEntry; N]) -> [Profile; N] {
    let mut variants = [Profile::Other; N];
    let mut index = 0;
    while index < N {
        variants[index] = rows[index].profile;
        index += 1;
    }
    variants
}

/// Fails the build unless every row sits at its own [`Profile`] discriminant, which
/// is what lets [`Profile::entry`] index [`PROFILE_TABLE`] directly.
const _: () = check_rows_match_discriminants(&PROFILE_ROWS);

const fn check_rows_match_discriminants<const N: usize>(rows: &[ProfileEntry; N]) {
    let mut index = 0;
    while index < N {
        assert!(
            rows[index].profile as usize == index,
            "PROFILE_ROWS must list each profile at its discriminant index"
        );
        index += 1;
    }
}

/// One row of the device profile table.
#[derive(Debug, Clone, Copy)]
pub struct ProfileEntry {
    pub profile: Profile,
    /// Canonical KCC profile code, e.g. `KV`.
    pub code: &'static str,
    /// Human-readable device name.
    pub label: &'static str,
    pub kind: DeviceKind,
    pub width: u32,
    pub height: u32,
    pub palette: &'static [u8],
}

/// The device profile rows, defined once as a `const` so [`ALL_PROFILES`] and the
/// discriminant check can be derived from them at compile time.
const PROFILE_ROWS: [ProfileEntry; 41] = [
    ProfileEntry {
        profile: Profile::K1,
        code: "K1",
        label: "Kindle 1",
        kind: DeviceKind::Kindle,
        width: 600,
        height: 670,
        palette: &PALETTE4,
    },
    ProfileEntry {
        profile: Profile::K2,
        code: "K2",
        label: "Kindle 2",
        kind: DeviceKind::Kindle,
        width: 600,
        height: 670,
        palette: &PALETTE15,
    },
    ProfileEntry {
        profile: Profile::Kdx,
        code: "KDX",
        label: "Kindle DX/DXG",
        kind: DeviceKind::Kindle,
        width: 824,
        height: 1000,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::K34,
        code: "K34",
        label: "Kindle Keyboard/Touch",
        kind: DeviceKind::Kindle,
        width: 600,
        height: 800,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::K57,
        code: "K57",
        label: "Kindle 5/7",
        kind: DeviceKind::Kindle,
        width: 600,
        height: 800,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::Kpw,
        code: "KPW",
        label: "Kindle Paperwhite 1/2",
        kind: DeviceKind::Kindle,
        width: 758,
        height: 1024,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::Kv,
        code: "KV",
        label: "Kindle Voyage",
        kind: DeviceKind::Kindle,
        width: 1072,
        height: 1448,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::Kpw34,
        code: "KPW34",
        label: "Kindle Paperwhite 3/4/Oasis",
        kind: DeviceKind::Kindle,
        width: 1072,
        height: 1448,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::K810,
        code: "K810",
        label: "Kindle 8/10",
        kind: DeviceKind::Kindle,
        width: 600,
        height: 800,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::Ko,
        code: "KO",
        label: "Kindle Oasis 2/3",
        kind: DeviceKind::Kindle,
        width: 1264,
        height: 1680,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::K11,
        code: "K11",
        label: "Kindle 11",
        kind: DeviceKind::Kindle,
        width: 1072,
        height: 1448,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::Kpw5,
        code: "KPW5",
        label: "Kindle Paperwhite 5/Signature Edition",
        kind: DeviceKind::Kindle,
        width: 1236,
        height: 1648,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::Kpw6,
        code: "KPW6",
        label: "Kindle Paperwhite 6",
        kind: DeviceKind::Kindle,
        width: 1272,
        height: 1696,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::Ks1860,
        code: "KS1860",
        label: "Kindle 1860",
        kind: DeviceKind::Kindle,
        width: 1860,
        height: 1920,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::Ks1920,
        code: "KS1920",
        label: "Kindle 1920",
        kind: DeviceKind::Kindle,
        width: 1920,
        height: 1920,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::Ks1240,
        code: "KS1240",
        label: "Kindle 1240",
        kind: DeviceKind::Kindle,
        width: 1240,
        height: 1860,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::Ks1324,
        code: "KS1324",
        label: "Kindle 1324",
        kind: DeviceKind::Kindle,
        width: 1324,
        height: 1986,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::Ks,
        code: "KS",
        label: "Kindle Scribe 1/2",
        kind: DeviceKind::Kindle,
        width: 1860,
        height: 2480,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::Kcs,
        code: "KCS",
        label: "Kindle Colorsoft",
        kind: DeviceKind::Kindle,
        width: 1272,
        height: 1696,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::Ks3,
        code: "KS3",
        label: "Kindle Scribe 3",
        kind: DeviceKind::Kindle,
        width: 1986,
        height: 2648,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::Kscs,
        code: "KSCS",
        label: "Kindle Scribe Colorsoft",
        kind: DeviceKind::Kindle,
        width: 1986,
        height: 2648,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::KoMt,
        code: "KoMT",
        label: "Kobo Mini/Touch",
        kind: DeviceKind::Kobo,
        width: 600,
        height: 800,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::KoG,
        code: "KoG",
        label: "Kobo Glo",
        kind: DeviceKind::Kobo,
        width: 768,
        height: 1024,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::KoGhd,
        code: "KoGHD",
        label: "Kobo Glo HD",
        kind: DeviceKind::Kobo,
        width: 1072,
        height: 1448,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::KoA,
        code: "KoA",
        label: "Kobo Aura",
        kind: DeviceKind::Kobo,
        width: 758,
        height: 1024,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::KoAhd,
        code: "KoAHD",
        label: "Kobo Aura HD",
        kind: DeviceKind::Kobo,
        width: 1080,
        height: 1440,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::KoAh2o,
        code: "KoAH2O",
        label: "Kobo Aura H2O",
        kind: DeviceKind::Kobo,
        width: 1080,
        height: 1430,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::KoAo,
        code: "KoAO",
        label: "Kobo Aura ONE",
        kind: DeviceKind::Kobo,
        width: 1404,
        height: 1872,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::KoN,
        code: "KoN",
        label: "Kobo Nia",
        kind: DeviceKind::Kobo,
        width: 758,
        height: 1024,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::KoC,
        code: "KoC",
        label: "Kobo Clara HD/Kobo Clara 2E",
        kind: DeviceKind::Kobo,
        width: 1072,
        height: 1448,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::KoCc,
        code: "KoCC",
        label: "Kobo Clara Colour",
        kind: DeviceKind::Kobo,
        width: 1072,
        height: 1448,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::KoL,
        code: "KoL",
        label: "Kobo Libra H2O/Kobo Libra 2",
        kind: DeviceKind::Kobo,
        width: 1264,
        height: 1680,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::KoLc,
        code: "KoLC",
        label: "Kobo Libra Colour",
        kind: DeviceKind::Kobo,
        width: 1264,
        height: 1680,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::KoF,
        code: "KoF",
        label: "Kobo Forma",
        kind: DeviceKind::Kobo,
        width: 1440,
        height: 1920,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::KoS,
        code: "KoS",
        label: "Kobo Sage",
        kind: DeviceKind::Kobo,
        width: 1440,
        height: 1920,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::KoE,
        code: "KoE",
        label: "Kobo Elipsa",
        kind: DeviceKind::Kobo,
        width: 1404,
        height: 1872,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::Rmk1,
        code: "Rmk1",
        label: "reMarkable 1",
        kind: DeviceKind::Remarkable,
        width: 1404,
        height: 1872,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::Rmk2,
        code: "Rmk2",
        label: "reMarkable 2",
        kind: DeviceKind::Remarkable,
        width: 1404,
        height: 1872,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::RmkPp,
        code: "RmkPP",
        label: "reMarkable Paper Pro",
        kind: DeviceKind::Remarkable,
        width: 1620,
        height: 2160,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::RmkPpMove,
        code: "RmkPPMove",
        label: "reMarkable Paper Pro Move",
        kind: DeviceKind::Remarkable,
        width: 954,
        height: 1696,
        palette: &PALETTE16,
    },
    ProfileEntry {
        profile: Profile::Other,
        code: "OTHER",
        label: "Other",
        kind: DeviceKind::Other,
        width: 0,
        height: 0,
        palette: &PALETTE16,
    },
];

/// The device profile table (see docs/cli.md), in [`ALL_PROFILES`] order.
pub static PROFILE_TABLE: [ProfileEntry; PROFILE_ROWS.len()] = PROFILE_ROWS;

/// Resolved profile geometry for a run.
///
/// This is the Rust counterpart of KCC's `options.profileData` tuple: the
/// profile's display name, target resolution, quantisation palette and gamma.
/// A `--custom-width`/`--custom-height` override produces a `Custom` instance
/// rather than a row in [`PROFILE_TABLE`].
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileData {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub palette: &'static [u8],
    pub gamma: f32,
}

impl Profile {
    /// The table row describing this profile.
    ///
    /// The discriminant indexes [`PROFILE_TABLE`] directly (O(1), no fallback row and
    /// no per-variant match); the `PROFILE_ROWS` check above proves every row sits at
    /// its own variant's discriminant, so the index is in range by construction
    /// (REFACTOR.md E11).
    pub fn entry(self) -> &'static ProfileEntry {
        &PROFILE_TABLE[self as usize]
    }

    /// Canonical KCC profile code (e.g. `KV`).
    pub fn code(self) -> &'static str {
        self.entry().code
    }

    /// Human-readable device name (e.g. `Kindle Voyage`).
    pub fn label(self) -> &'static str {
        self.entry().label
    }

    /// The device family this profile belongs to.
    pub fn device_kind(self) -> DeviceKind {
        self.entry().kind
    }

    /// True when the profile is a Kindle device (KCC's `iskindle`).
    pub fn is_kindle(self) -> bool {
        self.device_kind() == DeviceKind::Kindle
    }

    /// True when the profile is a reMarkable device.
    pub fn is_remarkable(self) -> bool {
        self.device_kind() == DeviceKind::Remarkable
    }

    /// KCC keys several behaviours off a `Ko` prefix, which is exactly the
    /// [`DeviceKind::Kobo`] family.
    pub fn is_kobo_brand(self) -> bool {
        self.device_kind() == DeviceKind::Kobo
    }

    /// True for the Scribe devices (KCC's `KS` prefix).
    pub fn is_scribe(self) -> bool {
        matches!(
            self,
            Profile::Ks1860
                | Profile::Ks1920
                | Profile::Ks1240
                | Profile::Ks1324
                | Profile::Ks
                | Profile::Ks3
                | Profile::Kscs
        )
    }

    /// Look up a profile by its canonical code (case-sensitive).
    pub fn from_code(code: &str) -> Option<Profile> {
        PROFILE_TABLE
            .iter()
            .find(|entry| entry.code == code)
            .map(|entry| entry.profile)
    }

    /// The default [`ProfileData`] for this profile (before custom overrides).
    pub fn data(self) -> ProfileData {
        let entry = self.entry();
        ProfileData {
            name: entry.label.to_string(),
            width: entry.width,
            height: entry.height,
            palette: entry.palette,
            gamma: 1.0,
        }
    }
}

impl ValueEnum for Profile {
    fn value_variants<'a>() -> &'a [Self] {
        &ALL_PROFILES
    }

    fn to_possible_value(&self) -> Option<PossibleValue> {
        // Attaching the device name as the value's help makes `--help` render the
        // `--profile` list with the acronym expanded (e.g. `KPW  Kindle Paperwhite
        // 1/2`) instead of a bare `[possible values: …]` line.
        Some(PossibleValue::new(self.code()).help(self.label()))
    }
}

impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}
