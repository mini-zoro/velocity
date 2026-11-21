use super::*;
use std::{
    io::Read,
    path::PathBuf,
    sync::mpsc::{self, Receiver},
};

#[derive(Debug, Default)]
pub(super) struct Report {
    output: String,
    diagnostics: Vec<Diagnostic>,
}
impl Report {
    fn diagnostic(&mut self, path: &std::path::Path, line: usize, column: usize, message: String) {
        let output_line = self.diagnostics.len();
        self.output
            .push_str(&format!("{}:{line}:{column}: {message}\n", path.display()));
        self.diagnostics.push(Diagnostic {
            path: path.to_owned(),
            line,
            column,
            message,
            output_line,
        });
    }
}

impl Builds {
    pub(crate) fn start_syntax_tests(
        &mut self,
        workspace: &Workspace,
        panels: &mut Panels,
        args: &Value,
    ) -> Result<(), String> {
        let path = args["file"]
            .as_str()
            .map(PathBuf::from)
            .or_else(|| workspace.tabs[workspace.active].path.clone())
            .ok_or("Run Syntax Tests needs a saved syntax test file")?;
        let path = std::path::absolute(path).map_err(|e| e.to_string())?;
        let syntax = args["syntax"].as_str().map(str::to_owned);
        let assets = workspace.assets.clone();
        let options = vel_fileio::DecodeOptions::from_settings(&workspace.editor().preferences);
        let (send, receive) = mpsc::channel();
        std::thread::Builder::new()
            .name("velocity-syntax-tests".into())
            .spawn(move || {
                let mut report = Report::default();
                if let Err(error) =
                    evaluate(&path, &assets, &options, syntax.as_deref(), &mut report)
                {
                    report.diagnostic(&path, 1, 1, error);
                }
                let _ = send.send(report);
            })
            .map_err(|e| e.to_string())?;
        self.cancel(panels);
        self.task = None;
        self.syntax_report = Some(receive);
        self.diagnostics.clear();
        self.selected = None;
        self.syntax = None;
        self.word_wrap = false;
        panels.configure_output(workspace, None, false);
        panels.clear_output();
        panels.append_output_ui(UiText::tr("Running syntax tests…\n"));
        panels.build_results.clear();
        panels.build_output_offset = 0;
        panels.result_scroll.offset = 0.;
        panels.panel = Some(Panel::Output);
        panels.select_build_results();
        Ok(())
    }

    pub(super) fn poll_syntax_tests(&mut self, panels: &mut Panels) -> bool {
        let Some(receive): Option<&Receiver<Report>> = self.syntax_report.as_ref() else {
            return false;
        };
        let report = match receive.try_recv() {
            Ok(report) => report,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => Report {
                output: "Syntax test worker stopped.\n".into(),
                ..Report::default()
            },
        };
        self.syntax_report = None;
        panels.clear_output();
        panels.output = report.output;
        self.diagnostics = report.diagnostics;
        panels.build_results = self
            .diagnostics
            .iter()
            .map(|d| {
                (
                    d.output_line,
                    crate::cli::FileLocation {
                        path: d.path.clone(),
                        line: d.line,
                        column: d.column,
                    },
                )
            })
            .collect();
        true
    }
}

fn evaluate(
    path: &std::path::Path,
    assets: &crate::language::Assets,
    options: &vel_fileio::DecodeOptions,
    syntax: Option<&str>,
    report: &mut Report,
) -> Result<(), String> {
    const MAX_TEST_BYTES: u64 = 16 * 1024 * 1024;
    let file = vel_fileio::regular::read(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_TEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_TEST_BYTES {
        return Err("Syntax test exceeds 16 MiB".into());
    }
    let utf8_error = std::str::from_utf8(&bytes).err();
    let loaded = vel_fileio::decode_bytes(&bytes, options).map_err(|e| {
        if utf8_error.is_some() {
            format!("Cannot decode non-UTF-8 syntax test: {e}")
        } else {
            e
        }
    })?;
    if let Some(error) = utf8_error {
        let before = &bytes[..error.valid_up_to()];
        let prefix = std::str::from_utf8(before).expect("valid prefix");
        let (line, tail) = prefix
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .rsplit_once('\n')
            .map_or((1, prefix.chars().count()), |(head, tail)| {
                (head.split('\n').count() + 1, tail.chars().count())
            });
        report.diagnostic(
            path,
            line,
            tail + 1,
            format!(
                "Non-UTF-8 syntax test decoded as {}",
                loaded.format.encoding
            ),
        );
    }
    let syntax = syntax
        .map(|argument| {
            vel_highlight::detect_syntax(&assets.syntaxes, "", "", Some(argument))
                .map(|s| s.path.as_str())
                .ok_or_else(|| format!("Unknown test syntax: {argument}"))
        })
        .transpose()?;
    let result = vel_syntax::syntest::evaluate_with_syntax(
        &assets.syntaxes,
        &assets.metadata,
        &loaded.text.to_string(),
        &path.to_string_lossy(),
        syntax,
    );
    if let Some(error) = result.error.or(result.skip_reason) {
        report.diagnostic(path, 1, 1, error);
    }
    for failure in result.examples {
        report.diagnostic(
            path,
            failure.line.max(1),
            1,
            format!(
                "Expected {}; got {} (source line {}, column {})",
                failure.selector,
                failure.actual,
                failure.source_line,
                failure.column + 1
            ),
        );
    }
    report.output.push_str(&format!(
        "Syntax tests: {} passed, {} failed, {} skipped\n",
        result.counts.passed, result.counts.failed, result.counts.skipped
    ));
    Ok(())
}
