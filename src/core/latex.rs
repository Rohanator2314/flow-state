//! The LaTeX → PDF → images pipeline.
//!
//! [`compile`] is blocking and synchronous: the GUI runs it off the UI thread
//! via `Task::perform` + `spawn_blocking`. The configured compiler
//! (`pdflatex`/`xelatex`, from TeX Live) runs in the file's directory, then
//! `pdftoppm` (poppler-utils) rasterizes up to [`MAX_PAGES`] pages at
//! 144 dpi. Failures return the first `!` line of the TeX log plus context.
//! Both tools must be on `$PATH`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use image::DynamicImage;

/// Cap on rasterized PDF pages, to bound memory on huge documents.
pub const MAX_PAGES: usize = 50;

/// Compile `path` and rasterize the resulting PDF's pages.
pub fn compile(compiler: &str, path: &Path) -> Result<Vec<DynamicImage>, String> {
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
    let mut cmd = Command::new(compiler);
    // TeX prompts on stdin when it hits some errors; never let it block on
    // (or steal input from) the parent process.
    cmd.stdin(Stdio::null());
    cmd.args(["-interaction=nonstopmode", "-halt-on-error"]);
    if let Some(dir) = dir {
        cmd.current_dir(dir);
    }
    cmd.arg(path.file_name().ok_or("bad path")?);
    let output = cmd
        .output()
        .map_err(|e| format!("failed to run {compiler}: {e}"))?;
    if !output.status.success() {
        return Err(extract_tex_error(&String::from_utf8_lossy(&output.stdout)));
    }

    let pdf = path.with_extension("pdf");
    let ppm_prefix = path.with_extension("preview");
    let status = Command::new("pdftoppm")
        .stdin(Stdio::null())
        .args(["-png", "-r", "144", "-l", &MAX_PAGES.to_string()])
        .arg(&pdf)
        .arg(&ppm_prefix)
        .output()
        .map_err(|e| format!("failed to run pdftoppm: {e}"))?;
    if !status.status.success() {
        return Err(format!(
            "pdftoppm failed: {}",
            String::from_utf8_lossy(&status.stderr).trim()
        ));
    }

    // pdftoppm names files <prefix>-1.png, -2.png … (zero-padded to a fixed
    // width when there are many pages, so a lexical sort keeps page order).
    // A bare relative path has parent Some("") — treat that as the cwd too.
    let dir = pdf
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let stem = ppm_prefix
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("preview")
        .to_string();
    let mut pngs: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "png")
                && p.file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.starts_with(&stem))
        })
        .collect();
    pngs.sort();
    let mut pages = Vec::with_capacity(pngs.len());
    for png in &pngs {
        let img = image::open(png).map_err(|e| format!("failed to read preview image: {e}"))?;
        let _ = std::fs::remove_file(png);
        pages.push(img);
    }
    if pages.is_empty() {
        return Err("pdftoppm produced no image".to_string());
    }
    Ok(pages)
}

/// Pull the first error line (starting with `!`) plus context from a TeX log.
fn extract_tex_error(log: &str) -> String {
    let lines: Vec<&str> = log.lines().collect();
    if let Some(pos) = lines.iter().position(|l| l.starts_with('!')) {
        lines[pos..(pos + 6).min(lines.len())].join("\n")
    } else {
        lines
            .iter()
            .rev()
            .take(8)
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compile_pipeline_produces_page_images() {
        let dir = std::env::temp_dir().join("flow-state-test");
        std::fs::create_dir_all(&dir).unwrap();
        let tex = dir.join("doc.tex");
        std::fs::write(
            &tex,
            "\\documentclass{article}\\begin{document}one\\newpage two\\end{document}\n",
        )
        .unwrap();
        match compile("pdflatex", &tex) {
            Ok(pages) => {
                assert_eq!(pages.len(), 2, "both pages should be rasterized");
                assert!(pages[0].width() > 100 && pages[0].height() > 100);
            }
            Err(e) => panic!("compile failed: {e}"),
        }
    }

    #[test]
    fn tex_error_is_extracted() {
        let dir = std::env::temp_dir().join("flow-state-test-err");
        std::fs::create_dir_all(&dir).unwrap();
        let tex = dir.join("bad.tex");
        std::fs::write(&tex, "\\documentclass{article}\\begin{document}\\badmacro\n").unwrap();
        match compile("pdflatex", &tex) {
            Err(e) => assert!(e.contains('!'), "error should quote the TeX log: {e}"),
            Ok(_) => panic!("expected failure"),
        }
    }
}
