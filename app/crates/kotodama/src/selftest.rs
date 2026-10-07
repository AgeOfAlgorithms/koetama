//! --selftest: a build's native parts load - the window, sound, sherpa-onnx, ONNX Runtime with the shipped language
//! detector, HTTPS for the model downloads. Exit code 0 when all do; the CI runs it on each build (and on the
//! installed copy).
use kd_common::paths;
use std::time::Duration;

pub fn run() -> i32 {
    let mut failed = 0;
    let mut step =
        |name: &str, f: &dyn Fn() -> Result<String, String>| match std::panic::catch_unwind(
            std::panic::AssertUnwindSafe(f),
        ) {
            Ok(Ok(s)) => println!(
                "ok   {name}{}",
                if s.is_empty() {
                    String::new()
                } else {
                    format!(": {s}")
                }
            ),
            Ok(Err(e)) => {
                failed += 1;
                println!("FAIL {name}: {e}");
            }
            Err(_) => {
                failed += 1;
                println!("FAIL {name}: it panicked");
            }
        };
    println!(
        "{} {} selftest ({})",
        paths::APP_NAME,
        paths::VERSION,
        paths::app_root().display()
    );
    step("the window", &window);
    step("sound", &|| {
        Ok(format!(
            "{} output devices, {} microphones",
            kd_audio::output_devices().len(),
            kd_audio::input_devices().len()
        ))
    });
    step("sherpa-onnx (the speech models' engine)", &|| {
        // (the library loads and runs: its resampler, which needs no model)
        sherpa_check()
    });
    step("the language detector (ONNX Runtime)", &|| {
        kd_speech::init_onnxruntime()?;
        let m = kd_speech::Models::new(1, kd_common::null_log());
        m.load("langid")?;
        let p = m.lid_probs(&vec![0.0f32; 16000])?;
        Ok(format!(
            "{} ({} languages)",
            kd_speech::lid_dir()?.display(),
            p.len()
        ))
    });
    step("model downloads (HTTPS)", &|| {
        let d = std::env::temp_dir().join(format!("kotodama-selftest-{}", std::process::id()));
        std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
        let dest = d.join("tokens.txt");
        let url = format!(
            "{}/csukuangfj/sherpa-onnx-nemo-ctc-giga-am-v3-russian-2025-12-16/resolve/32a4c7cc81809bd132e2d935ab99e9e6ab47fbec/tokens.txt",
            kd_common::fetch::base_url()
        );
        kd_common::fetch::download(&url, &dest, &|_, _| {}, 2).map_err(|e| e.to_string())?;
        let n = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        let _ = std::fs::remove_dir_all(&d);
        Ok(format!("{n} bytes"))
    });
    step("updates (GitHub)", &|| {
        kd_update::check(Duration::from_secs(15)).map(|r| match r {
            Some(r) => format!("{} is out", r.version),
            None => "no newer release".into(),
        })
    });
    println!("{failed} failed");
    if failed > 0 {
        1
    } else {
        0
    }
}

fn sherpa_check() -> Result<String, String> {
    let r = sherpa_onnx::LinearResampler::create(48000, 16000).ok_or("sherpa-onnx did not load")?;
    let y = r.resample(&vec![0.0f32; 4800], true);
    Ok(format!("resampled 4800 samples to {}", y.len()))
}

/// A window opens and draws one frame, then closes.
fn window() -> Result<String, String> {
    use eframe::egui;
    struct One(u32);
    impl eframe::App for One {
        fn ui(&mut self, ui: &mut egui::Ui, _f: &mut eframe::Frame) {
            ui.label("selftest");
            self.0 += 1;
            if self.0 >= 2 {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ui.ctx().request_repaint();
        }
    }
    let r = crate::gui::renderer();
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([200.0, 80.0])
            .with_visible(false),
        renderer: r,
        ..Default::default()
    };
    eframe::run_native(
        "Kotodama selftest",
        opts,
        Box::new(|_| Ok(Box::new(One(0)))),
    )
    .map_err(|e| e.to_string())?;
    Ok(format!(
        "egui, {}",
        if r == eframe::Renderer::Wgpu {
            "wgpu"
        } else {
            "OpenGL"
        }
    ))
}
