//! UI adapter for asynchronous builds and output-panel result navigation.
use crate::{
    panels::{Panel, Panels},
    workspace::Workspace,
};
use vel_build::{BuildSystem, Diagnostic, Event, ResultParser, Task, Variables};
use vel_config::{Value, json};
#[derive(Debug, Default)]
pub struct Builds {
    pub task: Option<Task>,
    pub diagnostics: Vec<Diagnostic>,
    pub selected: Option<usize>,
    parser: Option<ResultParser>,
    pending: String,
    pub syntax: Option<String>,
    pub word_wrap: bool,
}
impl Builds {
    pub fn start(
        &mut self,
        workspace: &Workspace,
        panels: &mut Panels,
        args: &Value,
    ) -> Result<(), String> {
        if self.task.is_some() {
            return Err("A build is already running; cancel it before starting another".into());
        }
        let direct = args.get("cmd").is_some() || args.get("shell_cmd").is_some();
        let build = if direct {
            BuildSystem::from_value("exec", args.clone())?
        } else {
            let name = args["build_system"]
                .as_str()
                .or(workspace.build_system.as_deref());
            let scope = workspace
                .editor()
                .language
                .borrow()
                .as_ref()
                .map(|l| l.root.to_string())
                .unwrap_or_default();
            let roots: Vec<_> = workspace.folders.iter().map(|f| f.path.clone()).collect();
            let build = workspace
                .build_systems
                .iter()
                .find(|b| {
                    name.map_or_else(
                        || {
                            b.matches(
                                &scope,
                                workspace.tabs[workspace.active].path.as_deref(),
                                &roots,
                            )
                        },
                        |n| b.name == n,
                    )
                })
                .ok_or("No matching build system; choose Tools > Build System")?;
            if let Some(variant) = args["variant"].as_str() {
                build.variant(variant)?
            } else {
                build.clone()
            }
        };
        let packages = workspace.data_path().map(|p| p.join("Packages"));
        let variables = Variables::new(
            workspace.tabs[workspace.active].path.as_deref(),
            workspace.folders.first().map(|f| f.path.as_path()),
            workspace.project_path(),
            packages.as_deref(),
        );
        let exec = build.resolve(&variables)?;
        let task = Task::spawn(&exec)?;
        panels.configure_output(workspace, exec.syntax.as_deref(), exec.word_wrap);
        self.diagnostics.clear();
        self.selected = None;
        self.pending.clear();
        self.syntax = exec.syntax;
        self.word_wrap = exec.word_wrap;
        self.parser = Some(exec.parser);
        self.task = Some(task);
        panels.output = if exec.quiet {
            String::new()
        } else {
            format!("[{}]\n[dir: {}]\n", build.name, exec.working_dir.display())
        };
        // Parser output line indices exclude the optional banner.
        panels.build_output_offset = panels.output.lines().count();
        panels.panel = Some(Panel::Output);
        panels.result_scroll.offset = 0.;
        panels.build_results.clear();
        Ok(())
    }
    pub fn poll(&mut self, panels: &mut Panels) -> bool {
        let mut changed = false;
        let mut finished = false;
        for _ in 0..128 {
            let event = self.task.as_ref().and_then(|t| t.events.try_recv().ok());
            let Some(event) = event else {
                break;
            };
            changed = true;
            match event {
                Event::Output { stream: _, text } => {
                    self.pending.push_str(&text);
                    if self.pending.len() > 1024 * 1024 {
                        let at = self
                            .pending
                            .floor_char_boundary(self.pending.len() - 1024 * 1024);
                        self.pending.drain(..at);
                    }
                    while let Some(end) = self.pending.find('\n') {
                        let line = self.pending.drain(..=end).collect::<String>();
                        self.result(&line, panels);
                    }
                    panels.output.push_str(&text);
                }
                Event::Error(e) => panels.output.push_str(&format!("\n[Build error: {e}]\n")),
                Event::Finished {
                    code,
                    cancelled,
                    elapsed,
                } => {
                    let line = std::mem::take(&mut self.pending);
                    if !line.is_empty() {
                        self.result(&line, panels);
                    }
                    panels.output.push_str(&format!(
                        "\n[{} in {:.3}s; exit code: {}]\n",
                        if cancelled { "Cancelled" } else { "Finished" },
                        elapsed.as_secs_f64(),
                        code.map_or_else(|| "signal".into(), |n| n.to_string())
                    ));
                    finished = true;
                }
            }
        }
        // UI retention is bounded; process pipes continue to drain after the panel limit.
        const LIMIT: usize = 8 * 1024 * 1024;
        if panels.output.len() > LIMIT {
            let at = panels
                .output
                .floor_char_boundary(panels.output.len() - LIMIT);
            let lines = panels.output[..at].bytes().filter(|b| *b == b'\n').count();
            panels.output.drain(..at);
            panels.output_reset = true;
            for d in &mut self.diagnostics {
                d.output_line = d.output_line.saturating_sub(lines);
            }
            panels.build_output_offset = panels.build_output_offset.saturating_sub(lines);
        }
        panels.build_results = self
            .diagnostics
            .iter()
            .map(|d| {
                (
                    d.output_line + panels.build_output_offset,
                    crate::cli::FileLocation {
                        path: d.path.clone(),
                        line: d.line,
                        column: d.column,
                    },
                )
            })
            .collect();
        if finished {
            self.task = None;
        }
        changed
    }
    fn result(&mut self, line: &str, _panels: &mut Panels) {
        if let Some(diagnostic) = self.parser.as_mut().and_then(|p| p.parse(line)) {
            self.diagnostics.push(diagnostic);
        }
    }
    pub fn navigate(&mut self, workspace: &mut Workspace, forward: bool) -> Result<(), String> {
        if self.diagnostics.is_empty() {
            return Ok(());
        }
        let len = self.diagnostics.len();
        let index = self
            .selected
            .map_or(if forward { 0 } else { len - 1 }, |n| {
                (n as isize + if forward { 1 } else { -1 }).rem_euclid(len as isize) as usize
            });
        self.selected = Some(index);
        let d = &self.diagnostics[index];
        workspace.open(crate::cli::FileLocation {
            path: d.path.clone(),
            line: d.line,
            column: d.column,
        })
    }
    pub fn choices(workspace: &Workspace, run: bool) -> Vec<vel_config::ui::PaletteItem> {
        let mut out = Vec::new();
        if !run {
            out.push(vel_config::ui::PaletteItem {
                caption: "Build System: Automatic".into(),
                invocation: vel_config::keymap::Invocation {
                    command: "set_build_system".into(),
                    args: json!({"name":"Automatic"}),
                },
            });
        }
        for build in &workspace.build_systems {
            let variants = if run {
                std::iter::once(None)
                    .chain(build.variants().into_iter().map(Some))
                    .collect::<Vec<_>>()
            } else {
                vec![None]
            };
            for variant in variants {
                out.push(vel_config::ui::PaletteItem {
                    caption: format!(
                        "{}{}",
                        build.name,
                        variant
                            .as_ref()
                            .map_or(String::new(), |v| format!(" — {v}"))
                    ),
                    invocation: vel_config::keymap::Invocation {
                        command: if run { "build" } else { "set_build_system" }.into(),
                        args: if run {
                            json!({"build_system":build.name,"variant":variant})
                        } else {
                            json!({"name":build.name})
                        },
                    },
                });
            }
        }
        out
    }
}
