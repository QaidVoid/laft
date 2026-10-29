//! Configuration diagnostics on standard error: source excerpts with the
//! spans underlined for people, or one JSON object per diagnostic and line
//! for programs.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::ops::Range;

use codespan_reporting::diagnostic as cs;
use codespan_reporting::files::{self, Files, SimpleFile};
use codespan_reporting::term::termcolor::{ColorChoice, StandardStream};
use codespan_reporting::term::{self, Config};
use serde_json::json;
use stav::{Diagnostic, Diagnostics, FileId, Severity, Span};

use crate::cli::MessageFormat;
use crate::ui::Ui;

/// Writes every diagnostic of `report` to standard error in the format
/// `ui` asks for.
pub(crate) fn emit(ui: &Ui, report: &Diagnostics) {
    let mut sources = Sources::default();
    for item in report.iter() {
        let labels = item.diagnostic.labels.iter().filter_map(|label| label.file);
        for file in item.file.into_iter().chain(labels) {
            if let (Some(name), Some(text)) = (report.file_name(file), report.source(file)) {
                sources.0.insert(file.index(), SimpleFile::new(name, text));
            }
        }
    }
    for item in report.iter() {
        let file = item.file.map(FileId::index);
        emit_one(ui, &sources, file, &item.diagnostic);
    }
}

/// Writes diagnostics about one source text that stands alone, such as the
/// syntax errors of a file being formatted.
pub(crate) fn emit_source(ui: &Ui, name: &str, text: &str, diagnostics: &[Diagnostic]) {
    let sources = Sources(BTreeMap::from([(0, SimpleFile::new(name, text))]));
    for diagnostic in diagnostics {
        emit_one(ui, &sources, Some(0), diagnostic);
    }
}

fn emit_one(ui: &Ui, sources: &Sources<'_>, file: Option<usize>, diagnostic: &Diagnostic) {
    let file = file.filter(|file| sources.0.contains_key(file));
    match ui.message_format {
        MessageFormat::Human => {
            let color = if ui.colored_err() {
                ColorChoice::Always
            } else {
                ColorChoice::Never
            };
            let stream = StandardStream::stderr(color);
            let mut out = stream.lock();
            let _ = term::emit_to_write_style(
                &mut out,
                &Config::default(),
                sources,
                &human(sources, file, diagnostic),
            );
        }
        MessageFormat::Json => {
            let line = json_line(sources, file, diagnostic);
            let _ = writeln!(io::stderr().lock(), "{line}");
        }
    }
}

/// The files diagnostics point into, by the index of their [`FileId`].
#[derive(Default)]
struct Sources<'a>(BTreeMap<usize, SimpleFile<&'a str, &'a str>>);

impl Sources<'_> {
    fn get(&self, id: usize) -> Result<&SimpleFile<&str, &str>, files::Error> {
        self.0.get(&id).ok_or(files::Error::FileMissing)
    }

    fn position(&self, id: usize, offset: usize) -> (usize, usize) {
        self.location(id, offset)
            .map_or((0, 0), |l| (l.line_number, l.column_number))
    }
}

impl<'a> Files<'a> for Sources<'a> {
    type FileId = usize;
    type Name = &'a str;
    type Source = &'a str;

    fn name(&'a self, id: usize) -> Result<&'a str, files::Error> {
        Ok(self.get(id)?.name())
    }

    fn source(&'a self, id: usize) -> Result<&'a str, files::Error> {
        Ok(self.get(id)?.source())
    }

    fn line_index(&'a self, id: usize, byte_index: usize) -> Result<usize, files::Error> {
        self.get(id)?.line_index((), byte_index)
    }

    fn line_range(&'a self, id: usize, line_index: usize) -> Result<Range<usize>, files::Error> {
        self.get(id)?.line_range((), line_index)
    }
}

fn label_file(sources: &Sources<'_>, file: Option<usize>, label: &stav::Label) -> Option<usize> {
    label
        .file
        .map(FileId::index)
        .or(file)
        .filter(|file| sources.0.contains_key(file))
}

/// The notes of a diagnostic, then the messages of labels that point into
/// no known file.
fn notes<'d>(
    sources: &Sources<'_>,
    file: Option<usize>,
    diagnostic: &'d Diagnostic,
) -> Vec<&'d str> {
    let orphans = diagnostic
        .labels
        .iter()
        .filter(|label| label_file(sources, file, label).is_none())
        .map(|label| label.message.as_str());
    diagnostic
        .notes
        .iter()
        .map(String::as_str)
        .chain(orphans)
        .collect()
}

fn human(
    sources: &Sources<'_>,
    file: Option<usize>,
    diagnostic: &Diagnostic,
) -> cs::Diagnostic<usize> {
    let severity = match diagnostic.severity {
        Severity::Error => cs::Severity::Error,
        Severity::Warning => cs::Severity::Warning,
    };
    let primary = file.map(|file| cs::Label::primary(file, diagnostic.span.range()));
    let secondary = diagnostic.labels.iter().filter_map(|label| {
        let file = label_file(sources, file, label)?;
        Some(cs::Label::secondary(file, label.span.range()).with_message(&label.message))
    });
    let notes = notes(sources, file, diagnostic)
        .into_iter()
        .map(|note| format!("note: {note}"))
        .chain(diagnostic.help.iter().map(|help| format!("help: {help}")));
    cs::Diagnostic::new(severity)
        .with_code(diagnostic.code)
        .with_message(&diagnostic.message)
        .with_labels(primary.into_iter().chain(secondary).collect())
        .with_notes(notes.collect())
}

fn json_line(sources: &Sources<'_>, file: Option<usize>, diagnostic: &Diagnostic) -> String {
    let label = |file: usize, span: Span, message: &str, primary: bool| {
        let (line, col) = sources.position(file, span.start);
        let (end_line, end_col) = sources.position(file, span.end);
        json!({
            "file": sources.get(file).map_or("", |f| f.name()),
            "start": {"line": line, "col": col},
            "end": {"line": end_line, "col": end_col},
            "message": message,
            "primary": primary,
        })
    };
    let primary = file.map(|file| label(file, diagnostic.span, "", true));
    let secondary = diagnostic.labels.iter().filter_map(|l| {
        Some(label(
            label_file(sources, file, l)?,
            l.span,
            &l.message,
            false,
        ))
    });
    json!({
        "code": diagnostic.code,
        "severity": match diagnostic.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        },
        "message": diagnostic.message,
        "labels": primary.into_iter().chain(secondary).collect::<Vec<_>>(),
        "notes": notes(sources, file, diagnostic),
        "help": diagnostic.help,
    })
    .to_string()
}
