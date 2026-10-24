//! Recursive, the brand typeface, registered as one named egui family per
//! weight and axis combination the window uses. Recursive is a variable font
//! (axes MONO, CASL, wght, slnt, CRSV); each family pins its axes with
//! `FontTweak::coords`, and all of them share the same font bytes.
//!
//! Note the font's own default weight is 300; the CSS sets 440 on `body`.

use std::sync::{Arc, OnceLock};

use egui::epaint::text::{
    FallbackRequest, FontData, FontDefinitions, FontFamily, FontInsert, FontPriority, FontTweak,
    InsertFontFamily, VariationCoords,
};

static RECURSIVE: &[u8] = include_bytes!("../../../../resources/fonts/Recursive-latin.ttf");

/// The weights used by the CSS and the Tailwind classes in the TSX.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Weight {
    /// `font-normal` (table headers).
    Normal,
    /// The body text weight.
    Body,
    /// `.nav-item`, `.btn-subtle`.
    Medium,
    /// `font-semibold`, `.badge`.
    Semibold,
    /// `.btn`, `.link`, `.keycap`, the selected nav item.
    Strong,
    /// `font-[760]`: dialog and section titles.
    Heavy,
    /// `font-[840]`: page titles.
    Black,
}

impl Weight {
    pub const ALL: [Self; 7] = [
        Self::Normal,
        Self::Body,
        Self::Medium,
        Self::Semibold,
        Self::Strong,
        Self::Heavy,
        Self::Black,
    ];

    /// The `wght` axis value.
    pub fn value(self) -> f32 {
        match self {
            Self::Normal => 400.0,
            Self::Body => 440.0,
            Self::Medium => 560.0,
            Self::Semibold => 600.0,
            Self::Strong => 620.0,
            Self::Heavy => 760.0,
            Self::Black => 840.0,
        }
    }
}

fn family_name(weight: Weight, mono: bool) -> String {
    format!(
        "recursive-{}{}",
        if mono { "mono-" } else { "" },
        weight.value() as u32
    )
}

/// The egui family for a weight, sans (`MONO 0`) or mono (`MONO 1`, for key caps and code).
pub fn family(weight: Weight, mono: bool) -> FontFamily {
    // Interned once: egui families are keyed by `Arc<str>`.
    static NAMES: OnceLock<Vec<(Weight, bool, FontFamily)>> = OnceLock::new();
    let names = NAMES.get_or_init(|| {
        Weight::ALL
            .iter()
            .flat_map(|&w| [(w, false), (w, true)])
            .map(|(w, m)| (w, m, FontFamily::Name(family_name(w, m).into())))
            .collect()
    });
    names
        .iter()
        .find(|(w, m, _)| *w == weight && *m == mono)
        .map(|(_, _, f)| f.clone())
        .unwrap_or(FontFamily::Proportional)
}

/// The mark: `CASL 1`, `slnt -12`, `CRSV 1`, weight 880.
pub fn logo_family() -> FontFamily {
    static LOGO: OnceLock<FontFamily> = OnceLock::new();
    LOGO.get_or_init(|| FontFamily::Name("recursive-logo".into()))
        .clone()
}

fn face(wght: f32, mono: f32, casl: f32, slnt: f32, crsv: f32) -> FontData {
    FontData::from_static(RECURSIVE).tweak(FontTweak {
        coords: VariationCoords::new([
            (b"MONO", mono),
            (b"CASL", casl),
            (b"wght", wght),
            (b"slnt", slnt),
            (b"CRSV", crsv),
        ]),
        // Chromium on Windows draws this unhinted font without grid-fitting.
        hinting: Some(false),
        ..FontTweak::default()
    })
}

/// The font definitions: every named family, with `Proportional` and
/// `Monospace` pointing at the body weight so stock egui widgets match.
pub fn definitions() -> FontDefinitions {
    let mut defs = FontDefinitions::empty();
    for weight in Weight::ALL {
        for mono in [false, true] {
            let name = family_name(weight, mono);
            defs.font_data.insert(
                name.clone(),
                Arc::new(face(weight.value(), mono as u8 as f32, 0.0, 0.0, 0.5)),
            );
            defs.families.insert(family(weight, mono), vec![name]);
        }
    }
    defs.font_data.insert(
        "recursive-logo".into(),
        Arc::new(face(880.0, 0.0, 1.0, -12.0, 1.0)),
    );
    defs.families
        .insert(logo_family(), vec!["recursive-logo".into()]);
    defs.families.insert(
        FontFamily::Proportional,
        vec![family_name(Weight::Body, false)],
    );
    defs.families
        .insert(FontFamily::Monospace, vec![family_name(Weight::Body, true)]);
    defs
}

/// Installs the fonts, plus a lazy fallback to Segoe UI (Symbol) for the few
/// characters the Latin subset lacks (←, →, ✓). Those files are read only
/// when such a character is first drawn.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(definitions());
    ctx.add_font_provider(Arc::new(|request: &FallbackRequest<'_>| fallback(request)));
}

fn fallback(request: &FallbackRequest<'_>) -> Option<FontInsert> {
    static SEGOE: OnceLock<Option<Arc<Vec<u8>>>> = OnceLock::new();
    static SYMBOL: OnceLock<Option<Arc<Vec<u8>>>> = OnceLock::new();
    let first = request.cluster.chars().next()?;
    let windows = std::env::var_os("WINDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "C:\\Windows".into());
    // Letters and punctuation from Segoe UI; arrows, dingbats and the rest from Segoe UI Symbol.
    let symbolic = matches!(first as u32, 0x2190..=0x2BFF);
    let (name, cell, file) = if symbolic {
        ("segoe-ui-symbol", &SYMBOL, "seguisym.ttf")
    } else {
        ("segoe-ui", &SEGOE, "segoeui.ttf")
    };
    let bytes = cell
        .get_or_init(|| {
            std::fs::read(windows.join("Fonts").join(file))
                .ok()
                .map(Arc::new)
        })
        .clone()?;
    let data = FontData::from_blob(bytes, 0);
    Some(FontInsert::new(
        name,
        data,
        vec![InsertFontFamily {
            family: request.family.clone(),
            priority: FontPriority::Lowest,
        }],
    ))
}
