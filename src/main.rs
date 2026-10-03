use std::path::PathBuf;
use std::process::ExitCode;

use agent_hub::config::{ClientConfig, Config};
use agent_hub::error::{Error, Result};

const USAGE: &str = "\
usage:
  agent-hub [serve]              serve the hub over HTTP (the default)
  agent-hub mcp                  bridge stdio MCP to the hub named by HUB_URL
  agent-hub call <tool> [json]   call one tool and print its JSON result
  agent-hub tools                list the hub's tools
  agent-hub kb <command>         read and write the project knowledge base
  agent-hub config [flags]       inspect and validate configuration
  agent-hub enrol [why]          request enrolment and wait for operator approval
  agent-hub backup --out DIR     back up the store while no hub serves it
  agent-hub restore --from DIR   restore the store from a backup
  agent-hub check                check store integrity, and any manifest present
  agent-hub doctor [--data-dir DIR] report the store's health and identity
  agent-hub health [--url URL]   GET /readyz and exit non-zero when not ready

The hub is configured by config.toml and environment variables.
";

const CONFIG_USAGE: &str = "\
usage:
  agent-hub config            show every setting, its value, and where it came from
  agent-hub config --path     print the files that would be read, found or not
  agent-hub config --check    parse and validate, exit non-zero on a problem
";

const BACKUP_USAGE: &str = "\
usage:
  agent-hub backup --out DIR [--data-dir DIR]

Copy the store offline: the hub database, every session brain and project
knowledge file through the engine, every artifact blob verbatim, and the
embedded tailnet's key state, with a manifest.json of each file's size and
sha256. A running hub holds the engine lock, so stop it first, or snapshot the
volume.
";

const RESTORE_USAGE: &str = "\
usage:
  agent-hub restore --from DIR [--data-dir DIR] [--force]

Verify every checksum, then replace the data directory with the backup's
contents through a staging directory. Refuses a non-empty data directory
without --force, and refuses while a hub holds the store.
";

const CHECK_USAGE: &str = "\
usage:
  agent-hub check [--data-dir DIR]

Run the engine's integrity check on the hub database and every session brain
and project knowledge file, verify a backup manifest when one is present, and
report artifact blobs the store names but the tree is missing. Exits non-zero
on any failure.
";

const HEALTH_USAGE: &str = "\
usage:
  agent-hub health [--url URL]

GET the hub's /readyz probe and exit 0 when it is ready, non-zero otherwise.
The URL defaults to http://127.0.0.1:8080; the /readyz path is appended unless
the URL already names it. Plain HTTP only, for a container health check with no
shell to curl from.
";

const DOCTOR_USAGE: &str = "\
usage:
  agent-hub doctor [--data-dir DIR]

Report whether the store under the data directory is healthy: the applied
schema version against what this binary supports, the data directory's device
and inode, free space on its volume, the write-ahead log size, the persisted id
high-water mark, and the engine's integrity check with any artifact blob the
store names but the tree is missing. Exits non-zero on any failure, and refuses
while a hub holds the store.
";

#[cfg(feature = "client")]
const KB_USAGE: &str = "\
usage:
  agent-hub kb get [path]        print a page, /fs/index.md by default
  agent-hub kb put <path> <-|--file F>
                                 write a page from stdin or a file
  agent-hub kb list [path]       print one page path per line
  agent-hub kb delete <path>     delete a page

  --project <id>   the project, or the HUB_PROJECT setting
  --json           print the tool's JSON result instead
  --if-version <v> write only while the page still reads as that version

A path is a page of the knowledge base, so a path outside /fs is taken as
relative to it: 'runbooks/deploy.md' is '/fs/runbooks/deploy.md'.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // A serving hub defaults to info, so an operator who set nothing still sees
    // a failure (a failed sweep, a dropped tailnet, a failed checkpoint); the
    // other subcommands stay quiet unless RUST_LOG says otherwise, and every
    // log goes to stderr so the stdio MCP transport keeps stdout protocol-clean.
    // The engine's own error level is included: a storage failure (a full disk,
    // a bad page) is logged inside `turso_core`, and `agent_hub=info` alone
    // would leave an unattended hub silent about the one fault it is failing on.
    let default_filter = match args.first().map(String::as_str) {
        None | Some("serve") => "agent_hub=info,turso_core=error",
        _ => "error",
    };
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| default_filter.to_string());
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
        .with_writer(std::io::stderr)
        .init();
    match args.first().map(String::as_str) {
        None | Some("serve") => report(serve()),
        Some("mcp") => mcp(),
        Some("call") => call(&args[1..]),
        Some("tools") => tools(),
        Some("kb") => kb(&args[1..]),
        Some("config") => config_cmd(&args[1..]),
        Some("enrol") => enrol(&args[1..]),
        Some("backup") => backup_cmd(&args[1..]),
        Some("restore") => restore_cmd(&args[1..]),
        Some("check") => check_cmd(&args[1..]),
        Some("doctor") => doctor_cmd(&args[1..]),
        Some("health") => health_cmd(&args[1..]),
        Some("help" | "--help" | "-h") => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        // An unknown subcommand must not start a hub: a typo would otherwise
        // become a second engine process on the data directory.
        Some(other) => {
            eprintln!("agent-hub: unknown subcommand '{other}'");
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn config_cmd(args: &[String]) -> ExitCode {
    let env_lookup = |key: &str| std::env::var(key).ok();
    if args.len() > 1 {
        eprintln!("agent-hub config: unexpected argument '{}'", args[1]);
        eprint!("{CONFIG_USAGE}");
        return ExitCode::from(2);
    }
    match args.first().map(String::as_str) {
        None => match agent_hub::config::print_config(&env_lookup) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("agent-hub: {err}");
                ExitCode::from(78)
            }
        },
        Some("--path") => {
            agent_hub::config::print_config_paths(&env_lookup);
            ExitCode::SUCCESS
        }
        Some("--check") => match agent_hub::config::validate_configuration(&env_lookup) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("agent-hub: {err}");
                ExitCode::from(78)
            }
        },
        Some("--help" | "-h") => {
            print!("{CONFIG_USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("agent-hub config: unknown option '{other}'");
            eprint!("{CONFIG_USAGE}");
            ExitCode::from(2)
        }
    }
}

/// The flags the three offline data-lifecycle commands share.
#[derive(Default)]
struct OpsOptions {
    data_dir: Option<String>,
    out: Option<String>,
    from: Option<String>,
    force: bool,
}

impl OpsOptions {
    fn parse(args: &[String], usage: &str) -> std::result::Result<Self, String> {
        let mut options = Self::default();
        let mut args = args.iter();
        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--data-dir" => options.data_dir = Some(ops_value(&mut args, "--data-dir")?),
                "--out" => options.out = Some(ops_value(&mut args, "--out")?),
                "--from" => options.from = Some(ops_value(&mut args, "--from")?),
                "--force" => options.force = true,
                other => return Err(format!("unknown option '{other}'\n{usage}")),
            }
        }
        Ok(options)
    }
}

/// The value after a long option, for a command that is not the client.
fn ops_value(
    args: &mut std::slice::Iter<'_, String>,
    flag: &str,
) -> std::result::Result<String, String> {
    args.next()
        .map(String::to_string)
        .ok_or_else(|| format!("{flag} needs a value"))
}

/// The one flag `doctor` takes.
#[derive(Default)]
struct DoctorOptions {
    data_dir: Option<String>,
}

impl DoctorOptions {
    /// Parse the closed option list: only `--data-dir` is doctor's, so a flag
    /// another offline command takes is refused rather than ignored.
    fn parse(args: &[String], usage: &str) -> std::result::Result<Self, String> {
        let mut options = Self::default();
        let mut args = args.iter();
        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--data-dir" => options.data_dir = Some(ops_value(&mut args, "--data-dir")?),
                flag @ ("--out" | "--from" | "--force") => {
                    return Err(format!("doctor takes only --data-dir, not {flag}\n{usage}"));
                }
                other => return Err(format!("unknown option '{other}'\n{usage}")),
            }
        }
        Ok(options)
    }
}

/// The data directory the command acts on: the flag, or the configured one.
///
/// Only `data_dir` is resolved here, not the whole serve configuration: an
/// offline command is a store-only operation, so it must not require the admin
/// token a non-loopback serve needs. `agent-hub backup --out DIR` without
/// `--data-dir` works on a host whose bind is not loopback.
fn ops_data_dir(explicit: Option<String>) -> std::result::Result<PathBuf, String> {
    match explicit {
        Some(dir) => Ok(PathBuf::from(dir)),
        None => Config::data_dir_from_env().map_err(|err| err.to_string()),
    }
}

/// Copy the whole store into a fresh output directory.
fn backup_cmd(args: &[String]) -> ExitCode {
    let options = match OpsOptions::parse(args, BACKUP_USAGE) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("agent-hub backup: {message}");
            return ExitCode::from(2);
        }
    };
    let Some(out) = options.out else {
        eprintln!("agent-hub backup: --out DIR is required\n{BACKUP_USAGE}");
        return ExitCode::from(2);
    };
    let data_dir = match ops_data_dir(options.data_dir) {
        Ok(data_dir) => data_dir,
        Err(message) => {
            eprintln!("agent-hub: {message}");
            return ExitCode::from(78);
        }
    };
    let runtime = match runtime() {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("agent-hub: {err}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(agent_hub::ops::backup(
        &data_dir,
        std::path::Path::new(&out),
    )) {
        Ok(report) => {
            println!(
                "backed up {} files, {} bytes, at schema v{} to {}{}",
                report.files,
                report.bytes,
                report.schema_version,
                out,
                if report.used_fallback {
                    " (byte copy: the engine refused VACUUM INTO)"
                } else {
                    ""
                }
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("agent-hub: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Verify a backup and replace the data directory with it.
fn restore_cmd(args: &[String]) -> ExitCode {
    let options = match OpsOptions::parse(args, RESTORE_USAGE) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("agent-hub restore: {message}");
            return ExitCode::from(2);
        }
    };
    let Some(from) = options.from else {
        eprintln!("agent-hub restore: --from DIR is required\n{RESTORE_USAGE}");
        return ExitCode::from(2);
    };
    let data_dir = match ops_data_dir(options.data_dir) {
        Ok(data_dir) => data_dir,
        Err(message) => {
            eprintln!("agent-hub: {message}");
            return ExitCode::from(78);
        }
    };
    let runtime = match runtime() {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("agent-hub: {err}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(agent_hub::ops::restore(
        std::path::Path::new(&from),
        &data_dir,
        options.force,
    )) {
        Ok(report) => {
            println!(
                "restored {} files, {} bytes, to {}",
                report.files,
                report.bytes,
                data_dir.display()
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("agent-hub: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Check store integrity and any backup manifest present.
fn check_cmd(args: &[String]) -> ExitCode {
    let options = match OpsOptions::parse(args, CHECK_USAGE) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("agent-hub check: {message}");
            return ExitCode::from(2);
        }
    };
    let data_dir = match ops_data_dir(options.data_dir) {
        Ok(data_dir) => data_dir,
        Err(message) => {
            eprintln!("agent-hub: {message}");
            return ExitCode::from(78);
        }
    };
    let runtime = match runtime() {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("agent-hub: {err}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(agent_hub::ops::check(&data_dir)) {
        Ok(report) => {
            for problem in &report.problems {
                eprintln!("agent-hub: {problem}");
            }
            if report.is_ok() {
                println!("checked {} engine files, no problems", report.checked);
                ExitCode::SUCCESS
            } else {
                println!(
                    "checked {} engine files, {} problems",
                    report.checked,
                    report.problems.len()
                );
                ExitCode::FAILURE
            }
        }
        Err(err) => {
            eprintln!("agent-hub: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Report whether the store under the data directory is healthy.
fn doctor_cmd(args: &[String]) -> ExitCode {
    let options = match DoctorOptions::parse(args, DOCTOR_USAGE) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("agent-hub doctor: {message}");
            return ExitCode::from(2);
        }
    };
    let data_dir = match ops_data_dir(options.data_dir) {
        Ok(data_dir) => data_dir,
        Err(message) => {
            eprintln!("agent-hub: {message}");
            return ExitCode::from(78);
        }
    };
    let runtime = match runtime() {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("agent-hub: {err}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(agent_hub::ops::doctor(&data_dir)) {
        Ok(report) => {
            println!("data directory: {}", report.data_dir.display());
            println!("identity: {}", report.identity);
            if report.newer_than_binary {
                println!(
                    "schema: {} (binary supports up to {}; the store is newer)",
                    report.schema_version, report.supported_max
                );
            } else {
                println!(
                    "schema: {} (binary supports up to {})",
                    report.schema_version, report.supported_max
                );
            }
            match report.free_bytes {
                Some(bytes) => println!("free space: {bytes} bytes"),
                None => println!("free space: unknown"),
            }
            println!("write-ahead log: {} bytes", report.wal_bytes);
            match report.id_high_water {
                Some(mark) => println!("id high-water mark: {mark}"),
                None => println!("id high-water mark: none"),
            }
            for problem in &report.problems {
                eprintln!("agent-hub: {problem}");
            }
            if report.is_ok() {
                println!("checked {} engine files, no problems", report.checked);
                ExitCode::SUCCESS
            } else {
                println!(
                    "checked {} engine files, {} problems",
                    report.checked,
                    report.problems.len()
                );
                ExitCode::FAILURE
            }
        }
        Err(err) => {
            eprintln!("agent-hub: {err}");
            ExitCode::FAILURE
        }
    }
}

/// GET the hub's readiness probe, for a container whose runtime has no shell.
fn health_cmd(args: &[String]) -> ExitCode {
    let mut base = String::from("http://127.0.0.1:8080");
    let mut args = args.iter();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--url" => match args.next() {
                Some(value) => base = value.clone(),
                None => {
                    eprintln!("agent-hub health: --url needs a value\n{HEALTH_USAGE}");
                    return ExitCode::from(2);
                }
            },
            "--help" | "-h" => {
                print!("{HEALTH_USAGE}");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("agent-hub health: unknown option '{other}'\n{HEALTH_USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    match probe_readyz(&base) {
        Ok(()) => {
            println!("ready");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("agent-hub health: {message}");
            ExitCode::FAILURE
        }
    }
}

/// How long the probe waits for the hub to answer.
const HEALTH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Request `/readyz` over plain HTTP and succeed on a 200.
///
/// The runtime image is distroless: no shell and no curl, so the check is the
/// binary itself. Only `http` is spoken; a hub behind TLS is checked by the
/// orchestrator's own probe.
fn probe_readyz(base: &str) -> std::result::Result<(), String> {
    use std::io::{Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};

    let url = url::Url::parse(base).map_err(|err| format!("'{base}' is not a URL: {err}"))?;
    if url.scheme() != "http" {
        return Err(format!(
            "health speaks plain http, not '{}'; point it at the hub's own listener",
            url.scheme()
        ));
    }
    let host = url.host_str().ok_or("the URL has no host")?;
    let port = url.port_or_known_default().ok_or("the URL has no port")?;

    let mut path = url.path().trim_end_matches('/').to_string();
    if !path.ends_with("/readyz") {
        path.push_str("/readyz");
    }

    let mut addresses = (host, port)
        .to_socket_addrs()
        .map_err(|err| format!("{host}:{port} did not resolve: {err}"))?;
    let address = addresses
        .next()
        .ok_or_else(|| format!("{host}:{port} resolved to no address"))?;
    let mut stream = TcpStream::connect_timeout(&address, HEALTH_TIMEOUT)
        .map_err(|err| format!("could not reach {address}: {err}"))?;
    stream
        .set_read_timeout(Some(HEALTH_TIMEOUT))
        .map_err(|err| format!("could not bound the read: {err}"))?;
    stream
        .set_write_timeout(Some(HEALTH_TIMEOUT))
        .map_err(|err| format!("could not bound the write: {err}"))?;

    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nAccept: application/json\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|err| format!("could not send the probe: {err}"))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|err| format!("could not read the answer: {err}"))?;
    let status = response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or("the hub did not answer with an HTTP status")?;
    if status == 200 {
        Ok(())
    } else {
        let detail = response.split("\r\n\r\n").nth(1).unwrap_or_default().trim();
        Err(format!("not ready: the hub answered {status} {detail}"))
    }
}

/// Serve the hub over HTTP.
fn serve() -> Result<()> {
    let (config, runtime) = server_runtime()?;
    runtime.block_on(agent_hub::app::run(config))
}

/// Serve MCP on stdio: a proxy to a running hub, or the embedded hub.
fn mcp() -> ExitCode {
    let client = match ClientConfig::from_env() {
        Ok(client) => client,
        Err(err) => {
            eprintln!("agent-hub: {err}");
            return ExitCode::from(78);
        }
    };
    if client.url.is_some() {
        return proxy(client);
    }
    // The two modes differ in who the caller is: embedded stdio is the local
    // human admin over the whole data directory, where the proxy is whatever
    // the hub resolves the token to. So the mode a caller got is said out loud.
    eprintln!(
        "agent-hub mcp: standalone against the local data directory, as the local admin; \
         set HUB_URL to reach a running hub as the token's agent instead"
    );
    report(embedded())
}

/// Serve the embedded MCP surface over the local data directory.
fn embedded() -> Result<()> {
    let (config, runtime) = server_runtime()?;
    match runtime.block_on(agent_hub::mcp::serve_stdio(config)) {
        Ok(()) => Ok(()),
        Err(err) if err.is_locked() => Err(Error::Conflict(
            "a hub is already using this directory; set HUB_URL to reach it instead".to_string(),
        )),
        Err(err) => Err(err),
    }
}

/// Bridge stdio to the hub the settings name.
#[cfg(feature = "client")]
fn proxy(client: ClientConfig) -> ExitCode {
    let runtime = match runtime() {
        Ok(runtime) => runtime,
        Err(err) => return report(Err(err)),
    };
    match runtime.block_on(agent_hub::client::serve_stdio(client)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            failure.report();
            ExitCode::from(failure.exit_code())
        }
    }
}

#[cfg(not(feature = "client"))]
fn proxy(_client: ClientConfig) -> ExitCode {
    eprintln!(
        "agent-hub: HUB_URL names a hub to proxy to, but this binary was built without the \
         client feature"
    );
    ExitCode::from(78)
}

/// Call one tool and print its result as JSON on stdout.
///
/// The arguments are a JSON object on the command line, or on stdin when the
/// argument is `-`. A tool that takes none needs neither, so a hook calling
/// one is not left reading a stdin the harness never closes.
#[cfg(feature = "client")]
fn call(args: &[String]) -> ExitCode {
    use agent_hub::client::Failure;

    let Some(tool) = args.first() else {
        return fail(&Failure::Usage(
            "call needs a tool name: agent-hub call <tool> [json]".to_string(),
        ));
    };
    let mut arguments = match args.get(1).map(String::as_str) {
        None => serde_json::json!({}),
        Some(source) => {
            let text = match source {
                "-" => match std::io::read_to_string(std::io::stdin()) {
                    Ok(text) => text,
                    Err(err) => {
                        return fail(&Failure::Usage(format!("stdin could not be read: {err}")));
                    }
                },
                argument => argument.to_string(),
            };
            match serde_json::from_str(&text) {
                Ok(arguments) => arguments,
                Err(err) => {
                    return fail(&Failure::Usage(format!(
                        "the tool arguments are not JSON: {err}"
                    )));
                }
            }
        }
    };

    let (config, runtime) = match client_runtime() {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    // A project named in the settings is the default for every call that takes
    // one, matching `kb`, so a hook sets HUB_PROJECT once rather than repeating
    // it on each call. A tool that does not take `project_id` ignores the extra
    // field, and an argument already present wins.
    if let Some(project) = config.project.as_deref()
        && arguments.get("project_id").is_none()
        && let Some(object) = arguments.as_object_mut()
    {
        object.insert("project_id".to_string(), serde_json::json!(project));
    }
    emit(runtime.block_on(agent_hub::client::call(&config, tool, arguments)))
}

/// List the hub's tools and their descriptions.
#[cfg(feature = "client")]
fn tools() -> ExitCode {
    let (config, runtime) = match client_runtime() {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    emit(runtime.block_on(agent_hub::client::tools(&config)))
}

/// Read and write the project knowledge base from a flag-shaped command line.
///
/// A hook has no MCP client and no patience for hand-escaped JSON, so the four
/// knowledge tools get a short form over the same one-shot call. `kb get`
/// prints the page itself rather than its JSON, because the hook pipes it
/// straight into a context window.
#[cfg(feature = "client")]
fn kb(args: &[String]) -> ExitCode {
    use agent_hub::client::Failure;

    let Some(command) = args.first().map(String::as_str) else {
        return fail(&Failure::Usage(format!("kb needs a command\n{KB_USAGE}")));
    };
    let options = match KbOptions::parse(&args[1..]) {
        Ok(options) => options,
        Err(message) => return fail(&Failure::Usage(format!("{message}\n{KB_USAGE}"))),
    };

    let (config, runtime) = match client_runtime() {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    let Some(project) = options.project.clone().or_else(|| config.project.clone()) else {
        return fail(&Failure::Usage(
            "kb needs a project: pass --project <id> or set HUB_PROJECT".to_string(),
        ));
    };
    // The knowledge base is one store of pages, so every command names the
    // project store and a path under it; only the output shape differs.
    let mut arguments = serde_json::json!({"store": "project", "project_id": project});
    let page = options.path.as_deref().map(kb_path);
    let missing_page = || {
        fail(&Failure::Usage(format!(
            "kb {command} needs the page it acts on\n{KB_USAGE}"
        )))
    };

    // An option a command accepts and then ignores misleads: a version guard
    // on a delete reads as a conditional delete and would delete regardless.
    let takes_content = command == "put";
    let prints_a_view = matches!(command, "get" | "list");
    let unused = [
        (
            options.if_version.is_some() && !takes_content,
            "--if-version",
        ),
        (options.file.is_some() && !takes_content, "--file"),
        (options.stdin && !takes_content, "-"),
        (options.json && !prints_a_view, "--json"),
    ]
    .into_iter()
    .find_map(|(given, name)| given.then_some(name));
    if let Some(option) = unused {
        return fail(&Failure::Usage(format!(
            "kb {command} does not take {option}\n{KB_USAGE}"
        )));
    }

    match command {
        "get" => {
            arguments["path"] = page.unwrap_or_else(|| KB_INDEX.to_string()).into();
            let result = runtime.block_on(agent_hub::client::call(&config, "brain_get", arguments));
            if options.json {
                emit(result)
            } else {
                emit_page(result)
            }
        }
        "put" => {
            let content = match options.content() {
                Ok(content) => content,
                Err(message) => return fail(&Failure::Usage(format!("{message}\n{KB_USAGE}"))),
            };
            let Some(page) = page else {
                return missing_page();
            };
            arguments["path"] = page.into();
            arguments["content"] = content.into();
            if let Some(version) = options.if_version.as_deref() {
                arguments["if_version"] = version.into();
            }
            emit(runtime.block_on(agent_hub::client::call(&config, "brain_put", arguments)))
        }
        "list" => {
            // A listing with no path is the whole base, which the tool reads as
            // an absent path rather than a root one.
            if let Some(page) = page {
                arguments["path"] = page.into();
            }
            let result =
                runtime.block_on(agent_hub::client::call(&config, "brain_list", arguments));
            if options.json {
                emit(result)
            } else {
                emit_paths(result)
            }
        }
        "delete" => {
            let Some(page) = page else {
                return missing_page();
            };
            arguments["path"] = page.into();
            emit(runtime.block_on(agent_hub::client::call(&config, "brain_delete", arguments)))
        }
        other => fail(&Failure::Usage(format!(
            "unknown kb command '{other}'\n{KB_USAGE}"
        ))),
    }
}

/// The page a `kb get` with no path reads: the bundle's own entry point, which
/// is a curated listing, so a hook needs to know no paths at all.
#[cfg(feature = "client")]
const KB_INDEX: &str = "/fs/index.md";

/// The brain path a `kb` argument names.
///
/// The knowledge base holds pages under `/fs` and nothing else, so a path that
/// is not already there is taken as relative to it, leading slashes and all.
#[cfg(feature = "client")]
fn kb_path(path: &str) -> String {
    if path == "/fs" || path.starts_with("/fs/") {
        return path.to_string();
    }
    format!("/fs/{}", path.trim_start_matches('/'))
}

/// What a `kb` command line carried besides its command.
#[cfg(feature = "client")]
#[derive(Default)]
struct KbOptions {
    path: Option<String>,
    project: Option<String>,
    if_version: Option<String>,
    file: Option<String>,
    stdin: bool,
    json: bool,
}

#[cfg(feature = "client")]
impl KbOptions {
    /// Parse one positional path and the closed list of long options.
    fn parse(args: &[String]) -> std::result::Result<Self, String> {
        let mut options = Self::default();
        let mut args = args.iter();
        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--project" => options.project = Some(flag_value(&mut args, "--project")?),
                "--if-version" => {
                    let version = flag_value(&mut args, "--if-version")?;
                    // An empty guard is a shell variable that was never set.
                    // Sent on, it would come back as a conflict with a version
                    // nobody asked for.
                    if version.is_empty() {
                        return Err("--if-version needs a version, got an empty value".to_string());
                    }
                    options.if_version = Some(version);
                }
                "--file" => options.file = Some(flag_value(&mut args, "--file")?),
                "--json" => options.json = true,
                "-" => options.stdin = true,
                flag if flag.starts_with('-') => return Err(format!("unknown flag '{flag}'")),
                path if options.path.is_none() => options.path = Some(path.to_string()),
                extra => return Err(format!("unexpected argument '{extra}'")),
            }
        }
        Ok(options)
    }

    /// The page body to write, from the named file or from stdin.
    ///
    /// The source is always explicit: a `kb put` that read stdin by default
    /// would hang on a harness that never closes it.
    fn content(&self) -> std::result::Result<String, String> {
        match (self.file.as_deref(), self.stdin) {
            // Two sources for one body: whichever won, the caller meant the
            // other half of the time, and the write would still succeed.
            (Some(_), true) => Err(
                "kb put takes one page body: --file <file> or - for stdin, not both".to_string(),
            ),
            (Some("-"), _) | (None, true) => {
                std::io::read_to_string(std::io::stdin()).map_err(|err| format!("stdin: {err}"))
            }
            (Some(file), _) => {
                std::fs::read_to_string(file).map_err(|err| format!("{file}: {err}"))
            }
            (None, false) => {
                Err("kb put needs the page body: pass --file <file> or - for stdin".to_string())
            }
        }
    }
}

/// The value after a long option.
#[cfg(feature = "client")]
fn flag_value(
    args: &mut std::slice::Iter<'_, String>,
    flag: &str,
) -> std::result::Result<String, String> {
    args.next()
        .map(String::to_string)
        .ok_or_else(|| format!("{flag} needs a value"))
}

/// Print the page a read returned, and nothing around it.
///
/// A page that already ends in a newline arrives byte for byte; one that does
/// not gets a single newline, so whatever the hook prints next starts its own
/// line.
#[cfg(feature = "client")]
fn emit_page(
    result: std::result::Result<serde_json::Value, agent_hub::client::Failure>,
) -> ExitCode {
    match result {
        Ok(value) => {
            let content = value
                .get("content")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if content.ends_with('\n') {
                print!("{content}");
            } else {
                println!("{content}");
            }
            ExitCode::SUCCESS
        }
        Err(failure) => fail(&failure),
    }
}

/// Print one listed page path per line.
#[cfg(feature = "client")]
fn emit_paths(
    result: std::result::Result<serde_json::Value, agent_hub::client::Failure>,
) -> ExitCode {
    match result {
        Ok(value) => {
            let entries = value.get("entries").and_then(serde_json::Value::as_array);
            for path in entries
                .into_iter()
                .flatten()
                .filter_map(|entry| entry.get("path"))
                .filter_map(serde_json::Value::as_str)
            {
                println!("{path}");
            }
            ExitCode::SUCCESS
        }
        Err(failure) => fail(&failure),
    }
}

/// The client settings and a runtime, or the code that says why not.
#[cfg(feature = "client")]
fn client_runtime() -> std::result::Result<(ClientConfig, tokio::runtime::Runtime), ExitCode> {
    let config = ClientConfig::from_env().map_err(|err| {
        eprintln!("agent-hub: {err}");
        ExitCode::from(78)
    })?;
    let runtime = runtime().map_err(|err| report(Err(err)))?;
    Ok((config, runtime))
}

/// Print one JSON object on stdout, or report the failure on stderr.
#[cfg(feature = "client")]
fn emit(result: std::result::Result<serde_json::Value, agent_hub::client::Failure>) -> ExitCode {
    match result {
        Ok(value) => {
            println!("{value}");
            ExitCode::SUCCESS
        }
        Err(failure) => fail(&failure),
    }
}

#[cfg(feature = "client")]
fn fail(failure: &agent_hub::client::Failure) -> ExitCode {
    failure.report();
    ExitCode::from(failure.exit_code())
}

#[cfg(not(feature = "client"))]
fn call(_args: &[String]) -> ExitCode {
    without_client()
}

#[cfg(not(feature = "client"))]
fn tools() -> ExitCode {
    without_client()
}

#[cfg(not(feature = "client"))]
fn kb(_args: &[String]) -> ExitCode {
    without_client()
}

#[cfg(feature = "client")]
fn enrol(args: &[String]) -> ExitCode {
    let (config, runtime) = match client_runtime() {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    match runtime.block_on(agent_hub::client::enrol(&config, args)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            failure.report();
            ExitCode::from(failure.exit_code())
        }
    }
}

#[cfg(not(feature = "client"))]
fn enrol(_args: &[String]) -> ExitCode {
    without_client()
}

#[cfg(not(feature = "client"))]
fn without_client() -> ExitCode {
    eprintln!("agent-hub: this binary was built without the client feature");
    ExitCode::from(78)
}

/// The server configuration and the runtime, built in that order.
fn server_runtime() -> Result<(Config, tokio::runtime::Runtime)> {
    let config = Config::from_env()?;

    // The embedded tailnet is validated here, before the store opens: a key on
    // a build without the feature, or an enabled endpoint with no admin token,
    // is a configuration error and must not have the side effect of creating,
    // migrating and backing up a data directory first. `app::run` reads the
    // same value again after the store is open; this is the early gate.
    let tailnet = config.tailnet_from_env()?;
    if tailnet.enabled() && config.admin_token.is_none() {
        return Err(Error::Config(
            "HUB_ADMIN_TOKEN is required when the tailnet endpoint is enabled".to_string(),
        ));
    }

    // The embedded tailnet sits behind the library's own experimental guard.
    // Opt into it here, before the runtime starts, so enabling the feature is
    // the operator's acknowledgement and no extra variable is needed.
    if config.tailnet_requested() {
        // SAFETY: the sync start of the process, before the runtime and any
        // other thread start.
        unsafe {
            agent_hub::net::acknowledge_unstable();
        }
    }

    Ok((config, runtime()?))
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| Error::Config(format!("could not start the async runtime: {err}")))
}

/// Report a failed run and exit non-zero.
fn report(result: Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("agent-hub: {err}");
            ExitCode::FAILURE
        }
    }
}
