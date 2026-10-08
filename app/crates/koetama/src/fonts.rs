//! Fonts for the window: egui's own have no Chinese, Japanese or Korean, which the language names and what the player
//! says need - the system's fonts for them are added as fallbacks when they are there (nothing is shipped).
use eframe::egui::{FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

/// (file, index in a collection) of fonts to try, in order: one for each of Chinese, Japanese, Korean.
fn candidates() -> Vec<(std::path::PathBuf, u32)> {
    let mut out = Vec::new();
    if cfg!(windows) {
        let dir = std::env::var_os("WINDIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| "C:\\Windows".into())
            .join("Fonts");
        // (YaHei has the Chinese characters and kana; Malgun Gothic the Hangul; SimSun if YaHei is missing)
        for (f, i) in [("msyh.ttc", 0), ("malgun.ttf", 0), ("simsun.ttc", 0)] {
            out.push((dir.join(f), i));
        }
    } else {
        for f in [
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/AppleSDGothicNeo.ttc",
        ] {
            out.push((f.into(), 0));
        }
    }
    out
}

/// The system's UI faces (regular, semibold): Segoe UI on Windows; elsewhere egui's own stay first.
fn ui_faces() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    if !cfg!(windows) {
        return None;
    }
    let dir = std::env::var_os("WINDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "C:\\Windows".into())
        .join("Fonts");
    let (r, b) = (dir.join("segoeui.ttf"), dir.join("seguisb.ttf"));
    (r.exists() && b.exists()).then_some((r, b))
}

pub fn install(ctx: &eframe::egui::Context) {
    let mut defs = FontDefinitions::default();
    // the UI face first (egui's own after it: their symbols), and a semibold family for headings and labels
    let mut semibold: Vec<String> = Vec::new();
    if let Some((regular, bold)) = ui_faces() {
        if let (Ok(r), Ok(b)) = (std::fs::read(&regular), std::fs::read(&bold)) {
            defs.font_data
                .insert("ui".into(), Arc::new(FontData::from_owned(r)));
            defs.font_data
                .insert("ui-semibold".into(), Arc::new(FontData::from_owned(b)));
            defs.families
                .entry(FontFamily::Proportional)
                .or_default()
                .insert(0, "ui".into());
            semibold.push("ui-semibold".into());
        }
    }
    semibold.extend(
        defs.families
            .get(&FontFamily::Proportional)
            .cloned()
            .unwrap_or_default(),
    );
    defs.families
        .insert(FontFamily::Name("semibold".into()), semibold);
    let mut added = 0;
    for (path, index) in candidates() {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let mut data = FontData::from_owned(bytes);
        data.index = index;
        defs.font_data.insert(name.clone(), Arc::new(data));
        for fam in [
            FontFamily::Proportional,
            FontFamily::Monospace,
            FontFamily::Name("semibold".into()),
        ] {
            defs.families.entry(fam).or_default().push(name.clone()); // (after the others: Latin stays as it is)
        }
        added += 1;
        if added >= 2 {
            break;
        }
    }
    ctx.set_fonts(defs);
}
