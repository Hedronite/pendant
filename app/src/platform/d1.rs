//! The D1 probe page: pick a folder with the phone's own picker and see
//! what the app can count in it, and whether access is still there after
//! the app restarts. Reached by `pendant://d1` or `PENDANT_START=d1`; it is
//! a measuring page for the D1 decision, not a feature, and links nothing.
//!
//! Every result is also a `d1:` trace line, so a device run's log is the
//! record.

use nsh_base::nav_page;
use nsh_core::prelude::*;

use crate::folders::{self, FolderError, FolderReport};
use crate::net::live::trace;
use crate::theme;
use crate::Nav;

/// One line for a report, the same on screen and in the log.
pub fn report_line(report: &FolderReport) -> String {
    format!(
        "md_top={} md_total={} other={} folders={} posix_path={} persisted={} restored={}",
        report.md_top,
        report.md_total,
        report.other_files,
        report.folders,
        report.posix_path,
        report.persisted,
        report.restored
    )
}

fn outcome_line(what: &str, outcome: &Result<FolderReport, FolderError>) -> String {
    match outcome {
        Ok(report) => format!("{what}: {}", report_line(report)),
        Err(err) => format!("{what}: {err}"),
    }
}

/// Runs a probe to its end even if the page goes, and shows the result
/// while the page is still there.
fn probe(
    page: Owner,
    shown: Signal<String>,
    what: &'static str,
    run: impl std::future::Future<Output = Result<FolderReport, FolderError>> + 'static,
) {
    trace(&format!("d1: {what}: started"));
    crate::net::live::spawn_detached(async move {
        let line = outcome_line(what, &run.await);
        trace(&format!("d1: {line}"));
        if page.is_live() {
            shown.set(line);
        }
    });
}

fn action(id: &'static str, label: &'static str, press: impl Fn() + 'static) -> impl IntoElement {
    text(label)
        .test_id(id)
        .text_size(px(16.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(theme::TEAL))
        .on_press(press)
        .role(AccessibilityRole::Button)
}

pub fn d1(nav: Nav) -> Screen {
    let page = Owner::current();
    let picked = signal(String::from("No folder picked yet."));
    let kept = signal(String::from("Checking kept access…"));
    // On open: whatever access was kept from an earlier pick, which after
    // a restart is the persistence check.
    probe(page, kept, "remembered", folders::remembered());

    nav_page().title("D1 folders").child(
        view()
            .flex_1()
            .flex_col()
            .gap_4()
            .p_4()
            .bg(rgb(theme::GRAPHITE))
            .child(
                text("Pick a folder of notes to measure what this phone lets the app read.")
                    .text_size(px(14.0))
                    .text_color(rgb(theme::TEXT)),
            )
            .child(action("d1-pick", "Pick a folder", move || {
                probe(page, picked, "pick", folders::pick())
            }))
            .child(
                text(move || picked.get())
                    .test_id("d1-picked")
                    .text_size(px(13.0))
                    .text_color(rgb(theme::DIM)),
            )
            .child(action("d1-remembered", "Check kept access", move || {
                probe(page, kept, "remembered", folders::remembered())
            }))
            .child(
                text(move || kept.get())
                    .test_id("d1-kept")
                    .text_size(px(13.0))
                    .text_color(rgb(theme::DIM)),
            )
            .child(action("d1-done", "Done", move || nav.back())),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_reads_as_one_line() {
        let report = FolderReport {
            md_top: 25,
            md_total: 30,
            other_files: 3,
            folders: 1,
            posix_path: true,
            persisted: true,
            restored: false,
        };
        assert_eq!(
            report_line(&report),
            "md_top=25 md_total=30 other=3 folders=1 posix_path=true persisted=true restored=false"
        );
        assert_eq!(
            outcome_line("pick", &Err(FolderError::Cancelled)),
            "pick: no folder was picked"
        );
    }
}
