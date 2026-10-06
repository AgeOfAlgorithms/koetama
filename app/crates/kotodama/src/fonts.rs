//! Fonts for the window: egui's own have no Chinese, Japanese or Korean, which the language names and what the player
//! says need - the system's fonts for them are added as fallbacks when they are there (nothing is shipped).
use eframe::egui::{FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

/// (file, index in a collection) of fonts to try, in order: one for each of Chinese, Japanese, Korean.
fn candidates() -> Vec<(std::path::PathBuf, u32)> {
    let mut out = Vec::new();
    if cfg!(windows) {
        let dir = std::env::var_os("WINDIR").map(std::path::PathBuf::from).unwrap_or_else(|| "C:\\Windows".into()).join("Fonts");
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

pub fn install(ctx: &eframe::egui::Context) {
    let mut defs = FontDefinitions::default();
    let mut added = 0;
    for (path, index) in candidates() {
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let mut data = FontData::from_owned(bytes);
        data.index = index;
        defs.font_data.insert(name.clone(), Arc::new(data));
        for fam in [FontFamily::Proportional, FontFamily::Monospace] {
            defs.families.entry(fam).or_default().push(name.clone()); // (after egui's own: Latin stays as it is)
        }
        added += 1;
        if added >= 2 {
            break;
        }
    }
    ctx.set_fonts(defs);
}
