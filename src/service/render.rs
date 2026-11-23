//! The text of a service definition: the systemd user unit and the launchd
//! property list. Pure functions, so a test can render one and read it back.

use std::path::{Path, PathBuf};

use super::UnitSpec;

/// The launchd label, and the name the plist file carries.
pub const LAUNCHD_LABEL: &str = "dev.agync.agent-hub";
/// The systemd unit name.
pub const SYSTEMD_UNIT: &str = "agent-hub.service";

/// A path the definition names, as text it can carry.
///
/// A name with a control character cannot be written into a unit or a plist
/// safely, and a name that is not UTF-8 cannot be written into either.
fn text_of<'a>(path: &'a Path, what: &str) -> Result<&'a str, String> {
    let text = path
        .to_str()
        .ok_or_else(|| format!("the {what} path is not valid UTF-8: {}", path.display()))?;
    if text.chars().any(char::is_control) {
        return Err(format!(
            "the {what} path holds a control character and cannot go in a service definition"
        ));
    }
    Ok(text)
}

/// The `HUB_*` variables the hub reads a setting from, each of which would
/// override the pinned config file if the service manager's own environment
/// carried it.
fn hub_setting_vars() -> Vec<String> {
    agent_hub::config::HUB_KEYS
        .iter()
        .map(|key| format!("HUB_{}", key.to_ascii_uppercase()))
        .collect()
}

/// A value for a systemd directive: bare when it is plain, else double-quoted.
///
/// Inside a unit, `%` starts a specifier and is written `%%`. Quoted, a
/// backslash and a double quote are escaped. A `$` is left alone: an
/// `Environment=` value is never expanded, and an `ExecStart` that holds one
/// turns expansion off with the `:` prefix instead (see [`systemd_unit`]).
fn systemd_word(value: &str) -> String {
    let escaped = value.replace('%', "%%");
    let plain = escaped
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || "/._+:@,=-%".contains(ch));
    if plain && !escaped.is_empty() {
        return escaped;
    }
    let mut quoted = String::with_capacity(escaped.len() + 2);
    quoted.push('"');
    for ch in escaped.chars() {
        if ch == '"' || ch == '\\' {
            quoted.push('\\');
        }
        quoted.push(ch);
    }
    quoted.push('"');
    quoted
}

/// Read back one word [`systemd_word`] wrote: double quotes group, a backslash
/// escapes inside them, and `%%` is a percent.
fn systemd_unword(text: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    let mut quoted = false;
    while let Some(ch) = chars.next() {
        match ch {
            '"' => quoted = !quoted,
            '\\' if quoted => out.push(chars.next()?),
            '%' if chars.peek() == Some(&'%') => {
                chars.next();
                out.push('%');
            }
            other => out.push(other),
        }
    }
    (!quoted).then_some(out)
}

/// `Environment=KEY=value`, quoted as a whole when the value needs it.
fn systemd_environment(key: &str, value: &str) -> String {
    format!("Environment={}", systemd_word(&format!("{key}={value}")))
}

/// The systemd user unit for `agent-hub serve`.
///
/// `AGENT_HUB_MANAGED_BY` is pinned because every process under a systemd
/// service inherits `INVOCATION_ID`: the label must not depend on it. Every
/// `HUB_*` setting variable is unset, so a value in the user manager's own
/// environment cannot point the service at another store or token than the
/// pinned config file names.
pub fn systemd_unit(spec: &UnitSpec) -> Result<String, String> {
    let binary = text_of(&spec.binary, "binary")?;
    let config = text_of(&spec.config, "config")?;
    // systemd expands `$` in a command line; `:` turns that off, so a `$` in
    // the binary's path reaches the kernel as it is.
    let no_expansion = if binary.contains('$') { ":" } else { "" };
    let exec = format!("{no_expansion}{} serve", systemd_word(binary));
    Ok(format!(
        "\
# Written by agent-hub service install. Re-running it rewrites this file;
# put local changes in a drop-in: systemctl --user edit agent-hub
[Unit]
Description=Agent Hub
Documentation=https://hub.agync.dev/docs/operate/native-service.md
StartLimitIntervalSec=60
StartLimitBurst=5

[Service]
Type=exec
ExecStart={exec}
{config_env}
{managed_env}
{log_env}
UnsetEnvironment={unset}
WorkingDirectory=%h
Restart=on-failure
RestartSec=2s
# 78 is a configuration error and 2 a usage error: restarting cannot fix either.
RestartPreventExitStatus=78 2
# The hub drains for ten seconds on SIGTERM, then checkpoints the store.
TimeoutStopSec=20
KillSignal=SIGTERM
LimitNOFILE=65536
UMask=0077
NoNewPrivileges=yes
LockPersonality=yes
RestrictRealtime=yes
RestrictSUIDSGID=yes
SystemCallArchitectures=native
SystemCallFilter=@system-service
SystemCallErrorNumber=EPERM

[Install]
WantedBy=default.target
",
        config_env = systemd_environment("HUB_CONFIG", config),
        managed_env = systemd_environment("AGENT_HUB_MANAGED_BY", "systemd"),
        log_env = systemd_environment("RUST_LOG", "agent_hub=info,turso_core=error"),
        unset = hub_setting_vars().join(" "),
    ))
}

/// The config file a unit [`systemd_unit`] wrote pins, read back from its text.
pub fn systemd_pinned_config(unit: &str) -> Option<PathBuf> {
    unit.lines()
        .filter_map(|line| line.strip_prefix("Environment="))
        .filter_map(systemd_unword)
        .find_map(|pair| pair.strip_prefix("HUB_CONFIG=").map(PathBuf::from))
}

/// The config file a loaded unit runs with, read from the `Environment=` value
/// of `systemctl show`: space-separated words, each in double quotes when it
/// holds a space.
pub fn systemd_shown_config(environment: &str) -> Option<PathBuf> {
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut chars = environment.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => quoted = !quoted,
            '\\' if quoted => word.extend(chars.next()),
            ch if ch.is_whitespace() && !quoted => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            other => word.push(other),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
        .iter()
        .find_map(|pair| pair.strip_prefix("HUB_CONFIG=").map(PathBuf::from))
}

/// The log file a plist [`launchd_plist`] wrote names, read back from its text.
pub fn launchd_log_path(plist: &str) -> Option<PathBuf> {
    let rest = plist.split_once("<key>StandardOutPath</key><string>")?.1;
    let value = rest.split_once("</string>")?.0;
    Some(PathBuf::from(xml_unescape(value)))
}

/// The config file a loaded agent runs with, read from `launchctl print`.
pub fn launchd_shown_config(printed: &str) -> Option<PathBuf> {
    printed.lines().find_map(|line| {
        let (key, value) = line.trim().split_once("=>")?;
        (key.trim() == "HUB_CONFIG").then(|| PathBuf::from(value.trim()))
    })
}

/// Escape text for an XML element body or attribute.
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn xml_unescape(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The launchd property list for `agent-hub serve`, with the launchd log at
/// `launchd_log`.
///
/// launchd has no way to unset a variable, so every `HUB_*` setting variable
/// is set empty, which the hub reads as unset: a value in the launchd domain's
/// environment cannot override the pinned config file.
pub fn launchd_plist(spec: &UnitSpec, launchd_log: &Path) -> Result<String, String> {
    let binary = xml_escape(text_of(&spec.binary, "binary")?);
    let config = xml_escape(text_of(&spec.config, "config")?);
    let home = xml_escape(text_of(&spec.home, "home")?);
    let log = xml_escape(text_of(launchd_log, "log")?);
    let cleared: String = hub_setting_vars()
        .iter()
        .map(|name| format!("    <key>{name}</key><string></string>\n"))
        .collect();
    Ok(format!(
        "\
<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">
<plist version=\"1.0\">
<dict>
  <key>Label</key><string>{LAUNCHD_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{binary}</string>
    <string>serve</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>HUB_CONFIG</key><string>{config}</string>
    <key>AGENT_HUB_MANAGED_BY</key><string>launchd</string>
{cleared}  </dict>
  <key>WorkingDirectory</key><string>{home}</string>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ThrottleInterval</key><integer>10</integer>
  <key>ExitTimeOut</key><integer>20</integer>
  <key>ProcessType</key><string>Standard</string>
  <key>SoftResourceLimits</key><dict><key>NumberOfFiles</key><integer>65536</integer></dict>
  <key>StandardErrorPath</key><string>{log}</string>
  <key>StandardOutPath</key><string>{log}</string>
</dict>
</plist>
"
    ))
}

/// The config file a plist [`launchd_plist`] wrote pins, read back from its
/// text.
pub fn launchd_pinned_config(plist: &str) -> Option<PathBuf> {
    let rest = plist.split_once("<key>HUB_CONFIG</key><string>")?.1;
    let value = rest.split_once("</string>")?.0;
    Some(PathBuf::from(xml_unescape(value)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::StartsAt;
    use std::path::PathBuf;

    fn spec(binary: &str, config: &str) -> UnitSpec {
        UnitSpec {
            binary: PathBuf::from(binary),
            config: PathBuf::from(config),
            home: PathBuf::from("/hubhome/user"),
            starts_at: StartsAt::Boot,
        }
    }

    fn directive<'a>(unit: &'a str, name: &str) -> Vec<&'a str> {
        unit.lines()
            .filter_map(|line| line.strip_prefix(name)?.strip_prefix('='))
            .collect()
    }

    #[test]
    fn a_loaded_jobs_config_is_read_from_what_the_manager_prints() {
        assert_eq!(
            systemd_shown_config(
                "AGENT_HUB_MANAGED_BY=systemd \"HUB_CONFIG=/a dir/config.toml\" RUST_LOG=x"
            ),
            Some(PathBuf::from("/a dir/config.toml"))
        );
        assert_eq!(systemd_shown_config("RUST_LOG=x"), None);
        assert_eq!(
            launchd_shown_config(
                "dev.agync.agent-hub = {\n\tenvironment = {\n\t\tHUB_CONFIG => /m/config.toml\n\t}\n}\n"
            ),
            Some(PathBuf::from("/m/config.toml"))
        );
    }

    #[test]
    fn the_unit_carries_the_documented_directives() {
        let unit = systemd_unit(&spec(
            "/hubhome/user/.local/bin/agent-hub",
            "/hubhome/user/.config/agent-hub/config.toml",
        ))
        .expect("render");
        assert_eq!(
            directive(&unit, "ExecStart"),
            vec!["/hubhome/user/.local/bin/agent-hub serve"]
        );
        let environment = directive(&unit, "Environment");
        assert!(
            environment.contains(&"HUB_CONFIG=/hubhome/user/.config/agent-hub/config.toml"),
            "{environment:?}"
        );
        assert!(
            environment.contains(&"AGENT_HUB_MANAGED_BY=systemd"),
            "the manager label must not depend on INVOCATION_ID: {environment:?}"
        );
        assert_eq!(directive(&unit, "Type"), vec!["exec"]);
        assert_eq!(directive(&unit, "RestartPreventExitStatus"), vec!["78 2"]);
        assert_eq!(directive(&unit, "TimeoutStopSec"), vec!["20"]);
        assert_eq!(directive(&unit, "WantedBy"), vec!["default.target"]);
        assert!(
            !unit.contains("network-online.target"),
            "a user manager cannot order against system targets"
        );
        for sandbox in [
            "ProtectSystem",
            "ProtectHome",
            "PrivateTmp",
            "PrivateDevices",
        ] {
            assert!(
                directive(&unit, sandbox).is_empty(),
                "{sandbox} needs user namespaces a user manager may not have"
            );
        }
    }

    #[test]
    fn awkward_paths_read_back_exactly() {
        for name in [
            "/hubhome/a b/.local/bin/agent-hub",
            "/hubhome/us\"er/agent-hub",
            "/hubhome/100%/agent-hub",
            "/hubhome/$HOME/agent-hub",
            "/hubhome/back\\slash/agent-hub",
            "/hubhome/tab\u{e9}/agent-hub",
            "/hubhome/plain-user_1/.local/bin/agent-hub",
        ] {
            let unit = systemd_unit(&spec(name, &format!("{name}.toml"))).expect(name);
            let exec = directive(&unit, "ExecStart")[0];
            // A command line with a `$` in it turns expansion off with `:`;
            // one without keeps the plain form.
            let (binary, no_expansion) = match exec.strip_prefix(':') {
                Some(rest) => (rest, true),
                None => (exec, false),
            };
            assert_eq!(no_expansion, name.contains('$'), "{exec}");
            let binary = binary
                .strip_suffix(" serve")
                .expect("the verb follows the binary");
            assert_eq!(
                systemd_unword(binary).as_deref(),
                Some(name),
                "ExecStart for {name}: {exec}"
            );
            assert_eq!(
                systemd_pinned_config(&unit),
                Some(PathBuf::from(format!("{name}.toml")))
            );
        }
    }

    #[test]
    fn every_hub_setting_variable_is_unset_in_the_unit_and_cleared_in_the_plist() {
        let unit = systemd_unit(&spec("/b", "/c")).expect("render");
        let unset: Vec<&str> = directive(&unit, "UnsetEnvironment")
            .into_iter()
            .flat_map(str::split_whitespace)
            .collect();
        let plist = launchd_plist(&spec("/b", "/c"), Path::new("/l")).expect("render");
        for key in [
            "HUB_DATA_DIR",
            "HUB_BIND",
            "HUB_ADMIN_TOKEN",
            "HUB_LOG_FILE",
        ] {
            assert!(unset.contains(&key), "{key} unset: {unset:?}");
            assert!(
                plist.contains(&format!("<key>{key}</key><string></string>")),
                "{key} cleared: {plist}"
            );
        }
        assert_eq!(unset.len(), agent_hub::config::HUB_KEYS.len());
        assert!(
            !unset.contains(&"HUB_CONFIG"),
            "the pinned config is not unset"
        );
        assert_eq!(
            directive(&unit, "Documentation"),
            vec!["https://hub.agync.dev/docs/operate/native-service.md"]
        );
    }

    #[test]
    fn the_plist_pins_its_config_and_reads_it_back() {
        let config = "/macusers/a&b <c>/.config/agent-hub/config.toml";
        let plist = launchd_plist(&spec("/b", config), Path::new("/l")).expect("render");
        assert_eq!(launchd_pinned_config(&plist), Some(PathBuf::from(config)));
    }

    #[test]
    fn a_path_that_cannot_be_written_safely_is_refused() {
        assert!(systemd_unit(&spec("/hubhome/a\nExecStart=/bin/sh", "/c")).is_err());
        assert!(launchd_plist(&spec("/bin/ok", "/c\u{7}"), Path::new("/l")).is_err());
    }

    /// A small well-formedness check: tags balance and nothing is left open. The
    /// plist is checked with `plutil -lint` where that exists (macOS).
    pub(crate) fn assert_well_formed(xml: &str) {
        let mut stack: Vec<String> = Vec::new();
        let mut rest = xml;
        while let Some(open) = rest.find('<') {
            rest = &rest[open..];
            let close = rest.find('>').expect("a tag that closes");
            let tag = &rest[1..close];
            rest = &rest[close + 1..];
            if tag.starts_with('?') || tag.starts_with('!') {
                continue;
            }
            if let Some(name) = tag.strip_prefix('/') {
                assert_eq!(stack.pop().as_deref(), Some(name), "unbalanced </{name}>");
            } else if !tag.ends_with('/') {
                let name = tag.split_whitespace().next().expect("a tag name");
                stack.push(name.to_string());
            }
            assert!(
                !rest.split('<').next().unwrap_or("").contains('>'),
                "a stray > in text"
            );
        }
        assert!(stack.is_empty(), "unclosed tags: {stack:?}");
    }

    #[test]
    fn the_plist_is_well_formed_and_escapes_what_it_must() {
        let name = "/macusers/a&b <c>/.local/bin/agent-hub";
        let plist = launchd_plist(
            &spec(name, "/macusers/a&b <c>/.config/agent-hub/config.toml"),
            Path::new("/macusers/a&b <c>/Library/Logs/agent-hub/launchd.log"),
        )
        .expect("render");
        assert_well_formed(&plist);
        assert!(
            plist.contains("<string>/macusers/a&amp;b &lt;c&gt;/.local/bin/agent-hub</string>")
        );
        assert!(plist.contains("<key>Label</key><string>dev.agync.agent-hub</string>"));
        assert!(plist.contains("<key>AGENT_HUB_MANAGED_BY</key><string>launchd</string>"));
        assert!(plist.contains("<key>ProcessType</key><string>Standard</string>"));
        assert!(plist.contains("<string>serve</string>"));
    }
}
