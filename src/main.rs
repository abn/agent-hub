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

The hub is named by HUB_URL, HUB_TOKEN and HUB_AGENT_ID, in the environment
or in ~/.agent-hub/config (HUB_CONFIG names another file).
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
    // Logs go to stderr so the stdio MCP transport keeps stdout protocol-clean.
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("serve") => report(serve()),
        Some("mcp") => mcp(),
        Some("call") => call(&args[1..]),
        Some("tools") => tools(),
        Some("kb") => kb(&args[1..]),
        Some("help" | "--help" | "-h") => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        // A subcommand this binary does not know used to start a hub, which
        // turned a typo into a second engine process on the data directory.
        Some(other) => {
            eprintln!("agent-hub: unknown subcommand '{other}'");
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
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
    runtime.block_on(agent_hub::mcp::serve_stdio(config))
}

/// Bridge stdio to the hub the settings name.
#[cfg(feature = "client")]
fn proxy(client: ClientConfig) -> ExitCode {
    eprintln!(
        "agent-hub mcp: proxying stdio to {}, as the agent the token resolves to",
        client.url.as_deref().unwrap_or_default()
    );
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
    let arguments = match args.get(1).map(String::as_str) {
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

#[cfg(not(feature = "client"))]
fn without_client() -> ExitCode {
    eprintln!("agent-hub: this binary was built without the client feature");
    ExitCode::from(78)
}

/// The server configuration and the runtime, built in that order.
fn server_runtime() -> Result<(Config, tokio::runtime::Runtime)> {
    let config = Config::from_env()?;

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
