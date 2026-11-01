// header.rs
// Module: Palepale project header
// Build: Rust 1.98.1 / crossterm 0.29 / Windows, macOS, Linux

use std::path::Path;

use crate::text::styled::{Line, Style};

pub fn render(project: &Path, width: usize) -> Line {
    let name = project
        .file_name()
        .unwrap_or(project.as_os_str())
        .to_string_lossy();
    let mut line = Line::styled(env!("CARGO_PKG_NAME"), Style::fg("accent").bold());
    let project = Line::styled(name, Style::fg("muted"));
    if width >= line.width() + project.width() + 4 {
        let gap = width - line.width() - project.width();
        line.push(" ".repeat(gap), Style::default());
        line.extend(project);
    }
    line.truncate(width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_project_names_never_displace_the_brand_or_overflow() {
        let project = Path::new("a-project-name-that-will-not-fit");
        for width in [1, 7, 20, 80] {
            let line = render(project, width);
            assert!(line.width() <= width);
            if width >= 8 {
                assert!(line.text().starts_with("palepale"));
            }
        }
        assert!(
            render(project, 80)
                .text()
                .ends_with("a-project-name-that-will-not-fit")
        );
    }
}
