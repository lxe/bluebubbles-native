use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;

use crate::{api::Credentials, cache, organization::ChatOrganization, theme};

const APP_ID: &str = "co.lxe.BlueBubblesNative";
const DEFAULT_SIDEBAR_WIDTH: i32 = 316;
static FILE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct TrayOptions {
    pub minimize: bool,
    pub close: bool,
}

impl TrayOptions {
    pub fn enabled(self) -> bool {
        self.minimize || self.close
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(default)]
struct Preferences {
    server: Option<String>,
    sidebar_width: i32,
    chat_organization: BTreeMap<String, ChatOrganization>,
    tray: TrayOptions,
    notifications: bool,
    colorway: String,
    text_size: i32,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            server: None,
            sidebar_width: DEFAULT_SIDEBAR_WIDTH,
            chat_organization: BTreeMap::new(),
            tray: TrayOptions::default(),
            notifications: true,
            colorway: theme::Colorway::System.key().into(),
            text_size: theme::DEFAULT_TEXT_SIZE,
        }
    }
}

fn config_dir() -> PathBuf {
    gtk::glib::user_config_dir().join("bluebubbles-native")
}

fn read_preferences() -> Preferences {
    match cache::read_json(&config_dir().join("preferences.json")) {
        Ok(value) => value,
        Err(_) => {
            let server = std::fs::read_to_string(config_dir().join("server"))
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty());
            Preferences {
                server,
                ..Preferences::default()
            }
        }
    }
}

fn update_preferences(update: impl FnOnce(&mut Preferences)) -> std::io::Result<()> {
    let _lock = FILE_LOCK.lock().unwrap();
    let mut settings = read_preferences();
    update(&mut settings);
    cache::write_json(&config_dir().join("preferences.json"), &settings)?;
    let _ = std::fs::remove_file(config_dir().join("server"));
    Ok(())
}

pub fn sidebar_width() -> i32 {
    read_preferences().sidebar_width.clamp(205, 850)
}

pub fn save_sidebar_width(width: i32) {
    let _ = update_preferences(|settings| settings.sidebar_width = width);
}

pub fn tray_options() -> TrayOptions {
    read_preferences().tray
}

pub fn save_tray_options(tray: TrayOptions) -> std::io::Result<()> {
    update_preferences(|settings| settings.tray = tray)
}

pub fn notifications_enabled() -> bool {
    read_preferences().notifications
}

pub fn save_notifications(enabled: bool) -> std::io::Result<()> {
    update_preferences(|settings| settings.notifications = enabled)
}

pub fn appearance() -> (theme::Colorway, i32) {
    let preferences = read_preferences();
    (
        theme::Colorway::from_key(&preferences.colorway).unwrap_or_default(),
        preferences.text_size,
    )
}

pub fn save_colorway(colorway: theme::Colorway) -> std::io::Result<()> {
    update_preferences(|settings| settings.colorway = colorway.key().into())
}

pub fn save_text_size(size: i32) -> std::io::Result<()> {
    update_preferences(|settings| settings.text_size = size)
}

pub fn chat_organization(server: &str) -> ChatOrganization {
    read_preferences()
        .chat_organization
        .remove(server)
        .unwrap_or_default()
}

pub fn save_chat_organization(server: &str, organization: ChatOrganization) -> std::io::Result<()> {
    update_preferences(|settings| {
        settings
            .chat_organization
            .insert(server.to_owned(), organization);
    })
}

pub fn from_environment() -> Result<Option<Credentials>, String> {
    match (
        std::env::var("BLUEBUBBLES_NATIVE_SERVER"),
        std::env::var("BLUEBUBBLES_NATIVE_PASSWORD"),
    ) {
        (Err(_), Err(_)) => Ok(None),
        (Ok(server), Ok(password)) => Credentials::new(&server, password).map(Some),
        _ => Err("Set both BLUEBUBBLES_NATIVE_SERVER and BLUEBUBBLES_NATIVE_PASSWORD.".into()),
    }
}

pub fn saved_server() -> String {
    read_preferences().server.unwrap_or_default()
}

pub fn load() -> Result<Option<Credentials>, String> {
    if let Some(credentials) = from_environment()? {
        return Ok(Some(credentials));
    }
    let server = saved_server();
    if server.is_empty() {
        return Ok(None);
    }
    let password = secret_command("lookup", &server)
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|password| password.trim_end_matches(['\n', '\r']).to_owned())
        .ok_or("Unlock your keyring or enter the server password in Preferences.")?;
    if !config_dir().join("preferences.json").is_file() {
        let _ = update_preferences(|settings| settings.server = Some(server.clone()));
    }
    Credentials::new(&server, password).map(Some)
}

pub fn remember(credentials: &Credentials) -> Result<(), String> {
    let server = credentials.server.as_str();
    let mut process = secret_command("store", server)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Install secret-tool to remember the password in your keyring.".to_string())?;
    let written = process
        .stdin
        .take()
        .unwrap()
        .write_all(credentials.password.as_bytes());
    let saved = process.wait().is_ok_and(|status| status.success());
    if written.is_err() || !saved {
        return Err("The keyring could not save your password.".into());
    }

    let previous = saved_server();
    if update_preferences(|settings| settings.server = Some(server.to_owned())).is_err() {
        if previous != server {
            clear_secret(server);
        }
        return Err("Could not save the server address.".into());
    }
    if !previous.is_empty() && previous != server {
        clear_secret(&previous);
    }
    Ok(())
}

pub fn forget() -> Result<(), String> {
    let previous = saved_server();
    if previous.is_empty() {
        return Ok(());
    }
    if !clear_secret(&previous) {
        return Err("The keyring could not remove the saved password.".into());
    }
    update_preferences(|settings| settings.server = None)
        .map_err(|_| "Could not remove the saved server address.".into())
}

fn secret_command(operation: &str, server: &str) -> Command {
    let mut command = Command::new("secret-tool");
    command.arg(operation);
    if operation == "store" {
        command.arg("--label=BlueBubbles Native");
    }
    command.args(["app", APP_ID, "server", server]);
    command
}

fn clear_secret(server: &str) -> bool {
    secret_command("clear", server)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
