//! Live2D constants — verbatim port of `echobot/app/services/web_console/live2d/constants.py`.

use std::collections::{HashMap, HashSet};

/// Default lip-sync parameter ids, tried in order.
pub const DEFAULT_LIP_SYNC_PARAMETER_IDS: &[&str] =
    &["ParamMouthOpenY", "PARAM_MOUTH_OPEN_Y", "MouthOpenY"];

/// Default mouth-form parameter ids, tried in order.
pub const DEFAULT_MOUTH_FORM_PARAMETER_IDS: &[&str] =
    &["ParamMouthForm", "PARAM_MOUTH_FORM", "MouthForm"];

/// Source tag for workspace (user-uploaded) models.
pub const LIVE2D_SOURCE_WORKSPACE: &str = "workspace";
/// Source tag for built-in models.
pub const LIVE2D_SOURCE_BUILTIN: &str = "builtin";
/// Filename for per-model EchoBot annotations.
pub const LIVE2D_ANNOTATIONS_FILENAME: &str = "echobot.live2d.json";
/// Motion group name for auto-discovered motions.
pub const LIVE2D_AUTO_MOTION_GROUP: &str = "EchoBotAuto";
/// Motion group name for idle animations.
pub const LIVE2D_IDLE_MOTION_GROUP: &str = "EchoBotIdle";

/// File suffixes accepted in Live2D uploads.
pub fn allowed_live2d_upload_suffixes() -> HashSet<&'static str> {
    let mut s = HashSet::new();
    s.extend([
        ".json", ".moc3", ".png", ".jpg", ".jpeg", ".webp", ".gif", ".avif",
        ".wav", ".mp3", ".ogg", ".m4a",
    ]);
    s
}

/// Max number of files in a single Live2D upload.
pub const MAX_LIVE2D_UPLOAD_FILES: usize = 512;
/// Max total bytes in a single Live2D upload (200 MiB).
pub const MAX_LIVE2D_UPLOAD_TOTAL_BYTES: usize = 200 * 1024 * 1024;

/// Hotkey actions the web UI can render.
pub fn supported_hotkey_actions() -> HashSet<&'static str> {
    let mut s = HashSet::new();
    s.extend(["ToggleExpression", "TriggerAnimation", "RemoveAllExpressions"]);
    s
}

/// Maps raw trigger-name strings to canonical token names.
pub fn hotkey_token_map() -> HashMap<&'static str, &'static str> {
    let mut m = HashMap::new();
    m.insert("alt", "alt");
    m.insert("leftalt", "alt");
    m.insert("rightalt", "alt");
    m.insert("shift", "shift");
    m.insert("leftshift", "shift");
    m.insert("rightshift", "shift");
    m.insert("control", "control");
    m.insert("ctrl", "control");
    m.insert("leftcontrol", "control");
    m.insert("rightcontrol", "control");
    m.insert("command", "meta");
    m.insert("leftcommand", "meta");
    m.insert("rightcommand", "meta");
    m.insert("win", "meta");
    m.insert("leftwin", "meta");
    m.insert("rightwin", "meta");
    m.insert("meta", "meta");
    m.insert("tab", "tab");
    m.insert("space", "space");
    m.insert("spacebar", "space");
    m.insert("enter", "enter");
    m.insert("return", "enter");
    m.insert("escape", "escape");
    m.insert("esc", "escape");
    m.insert("backspace", "backspace");
    m.insert("delete", "delete");
    m.insert("insert", "insert");
    m.insert("home", "home");
    m.insert("end", "end");
    m.insert("pageup", "pageup");
    m.insert("pagedown", "pagedown");
    m.insert("arrowup", "arrowup");
    m.insert("uparrow", "arrowup");
    m.insert("arrowdown", "arrowdown");
    m.insert("downarrow", "arrowdown");
    m.insert("arrowleft", "arrowleft");
    m.insert("leftarrow", "arrowleft");
    m.insert("arrowright", "arrowright");
    m.insert("rightarrow", "arrowright");
    m.insert("minus", "minus");
    m.insert("equal", "equal");
    m.insert("comma", "comma");
    m.insert("period", "period");
    m.insert("slash", "slash");
    m.insert("backslash", "backslash");
    m.insert("semicolon", "semicolon");
    m.insert("quote", "quote");
    m.insert("backquote", "backquote");
    m.insert("capslock", "capslock");
    m
}

/// Display label overrides for shortcut tokens.
pub fn display_hotkey_token(token: &str) -> String {
    let map: HashMap<&str, &str> = {
        let mut m = HashMap::new();
        m.insert("alt", "Alt");
        m.insert("control", "Ctrl");
        m.insert("shift", "Shift");
        m.insert("meta", "Meta");
        m.insert("space", "Space");
        m.insert("tab", "Tab");
        m.insert("enter", "Enter");
        m.insert("escape", "Esc");
        m.insert("backspace", "Backspace");
        m.insert("delete", "Delete");
        m.insert("insert", "Insert");
        m.insert("home", "Home");
        m.insert("end", "End");
        m.insert("pageup", "PageUp");
        m.insert("pagedown", "PageDown");
        m.insert("arrowup", "Up");
        m.insert("arrowdown", "Down");
        m.insert("arrowleft", "Left");
        m.insert("arrowright", "Right");
        m.insert("minus", "-");
        m.insert("equal", "=");
        m.insert("comma", ",");
        m.insert("period", ".");
        m.insert("slash", "/");
        m.insert("backslash", "\\");
        m.insert("semicolon", ";");
        m.insert("quote", "'");
        m.insert("backquote", "`");
        m.insert("capslock", "CapsLock");
        m
    };
    if let Some(v) = map.get(token) {
        return (*v).to_string();
    }
    if let Some(digit) = token.strip_prefix("digit") {
        return digit.to_string();
    }
    if let Some(key) = token.strip_prefix("key") {
        return key.to_uppercase();
    }
    if let Some(np) = token.strip_prefix("numpad") {
        let title = {
            let mut chars = np.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().chain(chars.flat_map(|c| c.to_lowercase())).collect(),
            }
        };
        return format!("Numpad {title}");
    }
    if token.len() >= 2 && token.len() <= 3
        && token.starts_with('f')
        && token[1..].chars().all(|c| c.is_ascii_digit())
    {
        return token.to_uppercase();
    }
    token.to_string()
}
