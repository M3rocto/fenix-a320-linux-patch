// SPDX-License-Identifier: MIT
//! What has been done, and the one thing to do next. Shared by both front ends.
use crate::{
    core,
    error::{Error, Result},
    fsx,
    json::{self, Map, Value, json},
    process::{self, Cmd, Input, Output},
    py,
    system::{Progress, System},
    targets::Target,
};
use std::{
    ffi::OsStr,
    fs,
    os::unix::fs::MetadataExt,
    path::PathBuf,
    sync::{Arc, Mutex},
};

pub const DASHBOARD: &str = "https://fenixsim.com/dashboard/";

fn settings_ready(target: &dyn Target) -> bool {
    // Fenix writes this file at sign-in. persistancy.xml can follow later and has its own step.
    target
        .prefix()
        .join(core::CONFIG)
        .join("fenixConfig.xml")
        .is_file()
}

/// Fenix installers the user has probably just downloaded, newest first.
pub fn installers(system: &dyn System) -> Vec<String> {
    let home = system.home();
    let mut found: Vec<(PathBuf, f64)> = Vec::new();
    for folder in [home.join("Downloads"), home.join("Desktop"), home.clone()] {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        let mut matching = Vec::new();
        for entry in entries {
            let Ok(entry) = entry else {
                matching.clear();
                break;
            };
            let path = entry.path();
            let name = fsx::name(&path).to_lowercase();
            if fsx::suffix(&name) == ".exe" && name.contains("fenix") && path.is_file() {
                // Python sorts by the modification time as a float.
                let Ok(info) = fs::metadata(&path) else {
                    continue;
                };
                matching.push((path, info.mtime() as f64 + 1e-9 * info.mtime_nsec() as f64));
            }
        }
        found.extend(matching);
    }
    found.sort_by(|a, b| b.1.total_cmp(&a.1));
    found
        .into_iter()
        .map(|(path, _)| path.to_string_lossy().into_owned())
        .collect()
}

fn dialogs(system: &dyn System) -> [Vec<std::ffi::OsString>; 2] {
    [
        Vec::from(crate::argv![
            "zenity",
            "--file-selection",
            "--title=Official Fenix Installer",
            "--file-filter=Windows installer | *.exe *.EXE"
        ]),
        Vec::from(crate::argv![
            "kdialog",
            "--getopenfilename",
            system.home().join("Downloads"),
            "*.exe"
        ]),
    ]
}

/// A native file dialog when the desktop offers one; None when cancelled or unavailable.
pub fn pick_file(system: &dyn System) -> Result<Option<String>> {
    for command in dialogs(system) {
        let program = command[0].to_string_lossy().into_owned();
        if fsx::which(&program, system.var("PATH")).is_some() {
            let mut cmd = Cmd::new(command);
            cmd.stdin = Input::Null;
            cmd.output = Output::CaptureOutput;
            let finished = process::run(&cmd, &|| Ok(()))?;
            let chosen = py::decode(finished.stdout)?;
            let chosen = py::strip(&chosen);
            return Ok((!chosen.is_empty()).then(|| chosen.to_string()));
        }
    }
    Ok(None)
}

pub fn can_pick_file(system: &dyn System) -> bool {
    ["zenity", "kdialog"]
        .iter()
        .any(|program| fsx::which(program, system.var("PATH")).is_some())
}

fn get<'a>(status: &'a Value, key: &str) -> &'a Value {
    status
        .as_object()
        .and_then(|items| items.get(key))
        .unwrap_or(&Value::Null)
}

/// `status.get(key, default)` as text.
fn get_text(status: &Value, key: &str, default: &str) -> String {
    match status.as_object().and_then(|items| items.get(key)) {
        Some(value) => json::text(value),
        None => default.to_string(),
    }
}

fn step(id: &str, title: &str, done: bool, text: String, button: &str) -> Map<String, Value> {
    let mut step = Map::new();
    step.insert("id".into(), json!(id));
    step.insert("title".into(), json!(title));
    step.insert("done".into(), json!(done));
    step.insert("text".into(), json!(text));
    step.insert("button".into(), json!(button));
    step
}

/// Steps in order; exactly one is current unless everything is done or setup is blocked.
pub fn overview(target: &dyn Target) -> Result<Value> {
    let status = target.status()?;
    let state = get(&status, "state").as_str();
    let steam = target.kind() == "steam";
    let broken = !matches!(state, Some("available" | "installed" | "legacy"));
    let patched = json::truthy(get(&status, "installed")) || state == Some("legacy");
    let update = json::truthy(get(&status, "update_available"));
    let title = target.title();
    let simulator = title.split(" on ").next().unwrap_or(&title).to_string();
    let launch = if steam {
        "from Steam"
    } else {
        "through Flightdeck"
    };
    let mut steps = vec![step(
        "install",
        "Install the compatibility patch",
        patched,
        format!(
            "Copies the simulator's Windows profile, installs Microsoft .NET Framework 4.8 and the Fenix fixes. \
             The original profile is kept for Restore. {}Takes 5–15 minutes and needs an internet connection.",
            if steam {
                "A separate Proton for the simulator is set up; your other games are not affected. "
            } else {
                ""
            }
        ),
        "Install patch",
    )];
    if update {
        // The later steps stay as they were and count again once the update is in.
        steps[0].insert("title".into(), json!("Update the compatibility patch"));
        steps[0].insert("done".into(), json!(false));
        steps[0].insert("button".into(), json!("Update patch"));
        steps[0].insert(
            "text".into(),
            json!(format!(
                "Patch {} is installed; this installer brings {}. Fenix, your sign-in and its settings are kept. Close the simulator and Fenix first.",
                json::text(get(&status, "installed_version")),
                json::text(get(&status, "version"))
            )),
        );
    }
    if steam {
        let running = json::truthy(get(&status, "steam_running"));
        let tool = get_text(&status, "tool_title", "Proton Fenix A320");
        let mut select = step(
            "select",
            "Let Steam use the Fenix Proton",
            json::truthy(get(&status, "tool_selected")),
            format!(
                "{}The installer sets “{tool}” for {title}, as Properties → Compatibility would. Only this simulator is changed.",
                if running {
                    "Steam is running. Quit it completely (Steam → Exit), then continue. "
                } else {
                    ""
                }
            ),
            if running {
                "Check again"
            } else {
                "Set it in Steam"
            },
        );
        select.insert(
            "note".into(),
            json!(format!("By hand: restart Steam, open the simulator's Properties → Compatibility, force “{tool}”.")),
        );
        steps.push(select);
    }
    let fenix_installed = json::truthy(get(&status, "fenix_installed"));
    let configured = get(&status, "configured") == &Value::Bool(true);
    let mut installer = step(
        "installer",
        "Run the official Fenix Installer",
        fenix_installed,
        "Download the installer from your Fenix account, choose the file here and complete it as on Windows, \
         including the prerequisites it offers. Close it when it has finished."
            .to_string(),
        "Run Fenix Installer",
    );
    installer.insert("needs_file".into(), json!(true));
    installer.insert("link".into(), json!(DASHBOARD));
    steps.push(installer);
    steps.push(step(
        "open",
        "Open Fenix and sign in",
        fenix_installed && settings_ready(target),
        "Sign in and activate your aircraft. Then close Fenix; its settings are needed for the next step.".to_string(),
        "Open Fenix",
    ));
    steps.push(step(
        "configure",
        "Apply the display settings",
        configured,
        "Sets CPU rendering and Legacy FCU readouts, the configuration the cockpit displays were verified with, \
         and starts Fenix together with the simulator."
            .to_string(),
        "Apply settings",
    ));
    if configured && get(&status, "readouts") == &Value::Bool(false) {
        steps.push(step(
            "readouts",
            "Set the Legacy FCU readouts",
            false,
            format!(
                "Fenix has not created the file for this setting yet. Start {simulator} {launch}, load the Fenix A320 once and \
                 close the simulator. Then apply the setting; CPU rendering and autostart are already in place."
            ),
            "Apply setting",
        ));
    }
    let mut current: Option<String> = None;
    for step in &mut steps {
        // An earlier local setup is active and stays as it is.
        let mut done = state == Some("legacy") || step.get("done") == Some(&Value::Bool(true));
        if !broken && current.is_none() && !done {
            current = step.get("id").and_then(Value::as_str).map(str::to_string);
        }
        // A later step cannot be complete before an earlier one.
        done = done && current.is_none();
        step.insert("done".into(), json!(done));
    }
    let mut message = status
        .as_object()
        .and_then(|items| items.get("message"))
        .cloned()
        .unwrap_or_else(|| json!(""));
    if broken {
        if !json::truthy(&message) {
            message = json!(
                "A previous setup did not finish. Restore the original profile, then install again."
            );
        }
    } else if get(&status, "idle") == &Value::Bool(false) {
        message = json!("The simulator or Fenix is running. Close them before continuing.");
    }
    let mut view = Map::new();
    view.insert("title".into(), json!(title));
    view.insert("kind".into(), json!(target.kind()));
    view.insert(
        "steps".into(),
        Value::Array(steps.into_iter().map(Value::Object).collect()),
    );
    view.insert("current".into(), json!(current));
    view.insert("broken".into(), json!(broken));
    view.insert("message".into(), message);
    view.insert(
        "can_restore".into(),
        json!(json::truthy(get(&status, "can_restore"))),
    );
    view.insert("complete".into(), json!(!broken && current.is_none()));
    view.insert(
        "finish".into(),
        json!(format!("Start {simulator} {launch}. Fenix starts with it.")),
    );
    view.insert("update".into(), json!(update));
    Ok(Value::Object(view))
}

/// `urllib.parse.unquote(urllib.parse.urlparse(address).path)` for a `file://` address.
fn file_address_path(address: &str) -> String {
    let address: String = address
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .collect();
    let rest = address.strip_prefix("file://").unwrap_or(&address);
    let rest = &rest[..rest.find('#').unwrap_or(rest.len())];
    let rest = &rest[..rest.find('?').unwrap_or(rest.len())];
    let path = &rest[rest.find('/').unwrap_or(rest.len())..];
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = |offset: usize| {
            bytes
                .get(index + offset)
                .and_then(|byte| char::from(*byte).to_digit(16))
        };
        if bytes[index] == b'%'
            && let (Some(high), Some(low)) = (hex(1), hex(2))
        {
            out.push((high * 16 + low) as u8);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn perform(
    system: &dyn System,
    target: &dyn Target,
    action: &str,
    bundle: &OsStr,
    progress: Progress<'_>,
    executable: Option<&str>,
    proton: Option<&OsStr>,
) -> Result<()> {
    match action {
        "install" => target.install(bundle, progress, proton),
        "select" => target.select(progress),
        "installer" => {
            // Paths are often pasted with quotes or as a file:// address.
            let mut executable = py::strip(executable.unwrap_or(""))
                .trim_matches(['\'', '"'])
                .to_string();
            if executable.starts_with("file://") {
                executable = file_address_path(&executable);
            }
            if executable.is_empty() {
                return Err(Error::patch("Choose the Fenix Installer file first."));
            }
            if !fsx::expanduser(&executable, system.var("HOME")).is_file() {
                return Err(Error::patch(format!(
                    "This file does not exist: {executable}"
                )));
            }
            target.app(progress, Some(OsStr::new(&executable)), false)?;
            progress("Fenix Installer closed.")
        }
        "open" => {
            target.app(progress, None, false)?;
            progress("Fenix closed.")
        }
        "manager" => target.app(progress, None, true),
        "configure" | "readouts" => target.configure(progress),
        "restore" => target.restore(progress),
        _ => Err(Error::patch("Unknown action")),
    }
}

/// Python: `except (core.PatchError, OSError, ValueError, KeyError, subprocess.SubprocessError)`.
pub fn shown_to_user(error: &Error) -> bool {
    error.is_reportable() || matches!(error, Error::Subprocess(_))
}

/// Where the terminal setup prints and asks.
pub trait Console: Send {
    fn print(&mut self, line: &str) -> Result<()>;
    fn input(&mut self, prompt: &str) -> Result<String>;
}

/// The same guided setup in a terminal.
pub fn text_wizard(
    system: &dyn System,
    found: &[Arc<dyn Target>],
    bundle: &OsStr,
    proton: Option<&OsStr>,
    console: &mut dyn Console,
) -> Result<()> {
    let Some(mut target) = found.first().cloned() else {
        return Err(Error::patch(
            "No simulator was found. Start MSFS once in Steam, or pass --steam-root or --runtime.",
        ));
    };
    if found.len() > 1 {
        for (number, item) in found.iter().enumerate() {
            console.print(&format!("  {}  {}", number + 1, item.label()))?;
        }
        let answer = console.input("Which simulator? [1] ")?;
        let choice = match py::strip(&answer) {
            "" => "1",
            choice => choice,
        };
        let number = choice
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| choice.parse::<usize>().ok())
            .flatten();
        target = match number.filter(|number| (1..=found.len()).contains(number)) {
            Some(number) => found[number - 1].clone(),
            None => return Err(Error::patch("No such simulator.")),
        };
    }
    loop {
        let view = overview(target.as_ref())?;
        let text = |value: &Value, key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let current = view
            .get("current")
            .and_then(Value::as_str)
            .map(str::to_string);
        let steps = view
            .get("steps")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        console.print(&format!(
            "\nFenix A320 Linux Patch · {}",
            text(&view, "title")
        ))?;
        for (number, step) in steps.iter().enumerate() {
            let mark = if step.get("done") == Some(&Value::Bool(true)) {
                "✓"
            } else if current.as_deref() == step.get("id").and_then(Value::as_str) {
                "→"
            } else {
                " "
            };
            console.print(&format!(" {mark} {}. {}", number + 1, text(step, "title")))?;
        }
        let message = view.get("message").cloned().unwrap_or(Value::Null);
        if json::truthy(&message) {
            console.print(&format!("\n! {}", json::text(&message)))?;
        }
        let step = steps.iter().find(|step| {
            current.is_some() && step.get("id").and_then(Value::as_str) == current.as_deref()
        });
        if view.get("complete") == Some(&Value::Bool(true)) {
            console.print(&format!("\nAll done. {}", text(&view, "finish")))?;
        } else if let Some(step) = step {
            console.print(&format!("\n{}", text(step, "text")))?;
            for key in ["note", "link"] {
                if step.get(key).is_some_and(json::truthy) {
                    console.print(&text(step, key))?;
                }
            }
        }
        let can_restore = view.get("can_restore") == Some(&Value::Bool(true));
        let prompt = format!(
            "\n{}{}[q] Quit: ",
            step.map(|step| format!("[Enter] {} · ", text(step, "button")))
                .unwrap_or_default(),
            if can_restore { "[r] Restore · " } else { "" }
        );
        let answer = py::strip(&console.input(&prompt)?).to_lowercase();
        if answer == "q" {
            return Ok(());
        }
        let action = if answer == "r" && can_restore {
            "restore".to_string()
        } else if let (Some(step), true) = (step, answer.is_empty()) {
            text(step, "id")
        } else {
            continue;
        };
        let mut executable = None;
        if action == "installer" {
            let detected = installers(system);
            let offered = detected
                .first()
                .map(|path| format!(" [{path}]"))
                .unwrap_or_default();
            let answer = console.input(&format!("Fenix Installer file{offered}: "))?;
            executable = match py::strip(&answer) {
                "" => detected.first().cloned(),
                answer => Some(answer.to_string()),
            };
        }
        let outcome = {
            let shared = Mutex::new(&mut *console);
            let print = |line: &str| match shared.lock() {
                Ok(mut console) => console.print(line),
                Err(_) => Err(Error::other("The terminal is no longer available.")),
            };
            perform(
                system,
                target.as_ref(),
                &action,
                bundle,
                &print,
                executable.as_deref(),
                proton,
            )
        };
        match outcome {
            Ok(()) => {}
            Err(error) if shown_to_user(&error) => console.print(&format!("\nError: {error}"))?,
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasted_file_addresses_become_paths() {
        assert_eq!(
            file_address_path("file:///home/a%20b/Fenix%C3%A4.exe"),
            "/home/a b/Fenix\u{e4}.exe"
        );
        assert_eq!(
            file_address_path("file://host/x.exe?query#fragment"),
            "/x.exe"
        );
        assert_eq!(file_address_path("file:///100%.exe"), "/100%.exe");
        assert_eq!(file_address_path("file://"), "");
    }
}
