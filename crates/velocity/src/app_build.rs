//! Build command hooks shared by native dispatch and headless tests.
use super::*;
impl App {
    pub(super) fn apply_build_action(
        &mut self,
        window: Option<&WindowHandle>,
        name: &str,
        args: &Value,
    ) -> Result<bool, String> {
        match name {
            "show_build_systems" => {
                vel_build::warm_login_environment();
                let items = crate::build::Builds::choices(&self.workspace, false);
                self.panels.choices(&mut self.workspace, items);
            }
            "new_build_system" => {
                self.workspace.new_file()?;
                let active = self.workspace.active;
                self.workspace.tabs[active].suggested_name = Some("untitled.sublime-build".into());
                self.workspace
                    .set_file_type("Packages/JSON/JSON.sublime-syntax")?;
                self.workspace.editor_mut().run("insert", &json!({"characters":"{\n    \"shell_cmd\": \"echo Build command\",\n    \"selector\": \"source\"\n}\n"}))?;
            }
            "set_build_system" => {
                vel_build::warm_login_environment();
                self.workspace.workbench_action(name, args)?;
                self.builds.clear_choices();
            }
            "build" | "exec" => {
                if args["cancel"].as_bool() == Some(true) || args["kill"].as_bool() == Some(true) {
                    if let Some(task) = &self.builds.task {
                        task.cancel();
                    }
                } else {
                    let direct = args.get("cmd").is_some() || args.get("shell_cmd").is_some();
                    let automatic = !direct
                        && args["build_system"].is_null()
                        && self.workspace.build_system.is_none();
                    let picker = args["select"].as_bool() == Some(true);
                    let from_picker = args["_build_picker"].as_bool() == Some(true);
                    let matching = if automatic || picker || from_picker {
                        crate::build::Builds::matching(&self.workspace)
                    } else {
                        vec![]
                    };
                    let remembered = automatic
                        .then(|| self.builds.remembered(&matching))
                        .flatten()
                        .cloned();
                    let choose =
                        picker || (automatic && remembered.is_none() && matching.len() > 1);
                    let mut resolved = args.clone();
                    if from_picker && let Some(name) = args["build_system"].as_str() {
                        self.builds.remember(
                            &matching,
                            name,
                            args["variant"].as_str().map(str::to_owned),
                        );
                    }
                    if automatic && !choose {
                        let (name, variant) = remembered
                            .or_else(|| matching.first().map(|b| (b.name.clone(), None)))
                            .ok_or("No matching build system; choose Tools > Build System")?;
                        resolved["build_system"] = json!(name);
                        resolved["variant"] = json!(variant);
                    }
                    if choose {
                        vel_build::warm_login_environment();
                        let items = crate::build::Builds::matching_choices(&matching);
                        self.panels.choices(&mut self.workspace, items);
                    } else {
                        // https://www.sublimetext.com/docs/build_systems.html (running builds).
                        // Save file-backed dirty tabs; untitled tabs have no build input path.
                        if name == "build"
                            && self
                                .workspace
                                .editor()
                                .settings
                                .bool("save_all_on_build", true)
                        {
                            let dirty = self
                                .workspace
                                .tabs
                                .iter()
                                .enumerate()
                                .filter(|(_, t)| t.path.is_some() && t.dirty())
                                .map(|(i, _)| i)
                                .collect::<Vec<_>>();
                            for index in dirty {
                                if let Some(window) = window {
                                    if !self.save_tab(window, index, false) {
                                        return Err("Build stopped: file was not saved".into());
                                    }
                                } else {
                                    self.workspace.save(index, None)?;
                                }
                            }
                        }
                        self.builds
                            .start(&self.workspace, &mut self.panels, &resolved)?;
                        self.panels.select_build_results();
                    }
                }
            }
            "cancel_build" => {
                if let Some(task) = &self.builds.task {
                    task.cancel();
                }
            }
            "next_result" | "prev_result" if window.is_none() => {
                if self.panels.search_results_active {
                    self.panels
                        .navigate_result(&mut self.workspace, name == "next_result")?;
                } else {
                    self.builds
                        .navigate(&mut self.workspace, name == "next_result")?;
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}
