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
  agent-hub project <command>    create, list, and delete projects
  agent-hub config [flags]       inspect and validate configuration
  agent-hub enrol [why]          request enrolment and wait for operator approval
  agent-hub backup --out DIR     back up the store while no hub serves it
  agent-hub backup --url URL     ask a running hub to back itself up
  agent-hub restore --from DIR   restore the store from a backup
  agent-hub check                check store integrity, and any manifest present
  agent-hub doctor [--data-dir DIR] report the store's health and identity
  agent-hub health [--url URL]   GET /readyz and exit non-zero when not ready

Every command prints its own usage with --help or -h, and reads no settings
and reaches nothing to do it.

The hub is configured by config.toml and environment variables.
";

const SERVE_USAGE: &str = "\
usage:
  agent-hub serve

Serve the hub over HTTP. With no subcommand, this is what runs. The bind, the
data directory, the admin token and the optional tailnet come from config.toml
and the environment; there are no flags, so an operator who asks a serving hub
for help gets its usage rather than a second process on the data directory.
";

const MCP_USAGE: &str = "\
usage:
  agent-hub mcp

Serve MCP on stdio. With HUB_URL set, this bridges stdio to that hub and holds
one connection for the life of the process, which is what keeps a session
active across calls. With no HUB_URL, it serves the local data directory
standalone as the human admin, which fails while a hub is serving that
directory.
";

const CALL_USAGE: &str = "\
usage:
  agent-hub call <tool> [json]
  agent-hub call <tool> -        read the arguments from stdin

Call one tool and print its JSON result on stdout. A tool that takes no
arguments needs neither, so a hook is not left reading a stdin nobody closes.
The exit code says what happened: 0 success, 1 a tool error, 2 usage, 69 the
hub is unreachable, 77 the token was refused, 78 nothing names a hub.
";

const TOOLS_USAGE: &str = "\
usage:
  agent-hub tools [--compact]

List the hub's tools with their descriptions and argument schemas, so CLI
discovery matches MCP discovery. The listing is printed indented, one tool per
block, because it is every schema in the hub and one line of it is unreadable.
--compact prints the same listing on one line, for a hook that wants it that
way.
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
  agent-hub backup --url URL

Copy the store offline: the hub database, every session brain and project
knowledge file through the engine, every artifact blob verbatim, and the
embedded tailnet's key state, with a manifest.json of each file's size and
sha256. A running hub holds the engine lock, so the offline copy refuses while
one serves the store.

With --url, the hub at URL takes the same backup itself while it serves, into a
new timestamped directory under its backup_dir setting (HUB_BACKUP_DIR), and
this prints where it landed. The request is the admin's: the token is
HUB_ADMIN_TOKEN, or admin_token under [hub] in config.toml, and HUB_TIMEOUT
bounds the wait. The hub chooses the directory, so --url takes no --out. The
exit code says what happened: 0 success, 1 the hub refused, 2 usage, 69 the hub
is unreachable, 77 the token was refused, 78 no admin token is set.
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

const KB_USAGE: &str = "\
usage:
  agent-hub kb get [path]        print a page, /fs/index.md by default
  agent-hub kb put <path> <-|--file F>
                                 write a page from stdin or a file
  agent-hub kb list [path]       print one page path per line
  agent-hub kb delete <path>     delete a page
  agent-hub kb history <path>    print the page's versions, newest first
  agent-hub kb revert <path> <version>
                                 put the page back to an earlier version
  agent-hub kb forget <path>     forget the page's earlier versions (admin)
  agent-hub kb export --dir D [--force]
                                 write every page into D as an OKF bundle
  agent-hub kb import --dir D [--dry-run] [--prune]
                                 write the pages of D that changed back

  --project <id>   the project, or the HUB_PROJECT setting
  --json           print the tool's JSON result instead
  --if-version <v> write only while the page still reads as that version
  --dir <D>        the folder an export writes or an import reads
  --force          export into a folder that is not empty, writing over its
                   page files and manifest and removing only the files of
                   pages its old manifest names and the hub no longer has
  --dry-run        list what an import would change and write nothing
  --prune          delete pages the export had and the folder no longer has

A path is a page of the knowledge base, so a path outside /fs is taken as
relative to it: 'runbooks/deploy.md' is '/fs/runbooks/deploy.md'.

A page is markdown with a YAML frontmatter block, which the knowledge base reads
and the human's wiki shows as the page's type, status and tags:

  ---
  type: Runbook              Concept, Guide, Runbook, Reference, Decision,
  title: Deploy the hub      Decision Record; the one field a page needs
  description: One line      what a reader gets here
  status: draft              draft or stable
  tags: [usage, ops]         free-form; a flow list, or one '- item' a line
  stale_after: 2027-01-01    optional; the page reads as stale after this date
  ---
  # Deploy the hub

The block opens when the page's first line is exactly ---, never '--- # comment'
or '---yaml', and holds plain 'key: value' lines. A page with no block is
stored as it was sent and comes back flagged with the minimum to add, so the
write stays lenient and nothing is lost to a missing field. Only the bundle
root carries okf_version; 'verified' and 'sources' are blocks the hub's own
review and promote write.

kb list walks the whole base under the path it is given, or the whole base when
it is given none, and prints pages only: every line is a page kb get can read.
--json is the tool's own result for that one listing instead, one level of
entries with their types, directories included.

kb history prints one line per write to the page, newest first: the version it
stored, when, who, and what it did, with 'current' on the version the page holds
now. A deleted page keeps its history. kb revert writes that version back as a
new write in the caller's name, so it restores a deleted page too and never
takes a version out of the history; --if-version guards it like a put, and
'absent' restores a deleted page only while it is still gone. --json on history
is the tool's result for the newest page of versions. Reverting to the version
the page already holds writes nothing.

kb delete removes the page but keeps its bytes in its history. kb forget is the
operator's purge: it removes the kept bytes of every version but the one the
page holds now, all of them for a deleted page, so they can no longer be read
or restored and their space is reused. The history's rows stay. It needs the
admin token as HUB_TOKEN; any other token gets the hub's refusal.

kb export writes each page at its path under /fs, with .agent-hub-kb.json
recording the project and each page's version, and refuses a folder that is not
empty unless --force is given; --force also removes the files of pages the old
manifest names that the hub no longer has. kb import lints the folder and
writes nothing when a page it would write has a lint error, then writes each
changed page guarded by the version the export recorded: a page changed on
both sides since then is a conflict, is skipped, and makes the exit code 1,
and one changed only on the hub is left as it is. Dot-named files and
directories are not pages, so the manifest and a .git directory stay out of the
bundle. A page missing from the folder is kept unless --prune is given.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // A help flag is a question about the command, not an argument to it, so it
    // is answered here, before anything else runs: no subcommand starts a hub,
    // opens the engine on the data directory or reaches the network to print its
    // own usage. One owner means every command answers the same way.
    if let Some(usage) = usage_for(&args) {
        print!("{usage}");
        return ExitCode::SUCCESS;
    }

    // A serving hub defaults to info, so an operator who set nothing still sees
    // a failure (a failed sweep, a dropped tailnet, a failed checkpoint); the
    // other subcommands stay quiet unless RUST_LOG says otherwise, and every
    // log goes to stderr so the stdio MCP transport keeps stdout protocol-clean.
    // The engine's own error level is included: a storage failure (a full disk,
    // a bad page) is logged inside `turso_core`, and `agent_hub=info` alone
    // would leave an unattended hub silent about the one fault it is failing on.
    // A client subcommand asks for less, because its stderr is the operator's
    // and carries what went wrong: its filter is the one that keeps the
    // transport's line about its own teardown out of it.
    let filter = match args.first().map(String::as_str) {
        None | Some("serve") => std::env::var("RUST_LOG")
            .unwrap_or_else(|_| "agent_hub=info,turso_core=error".to_string()),
        #[cfg(feature = "client")]
        _ => agent_hub::client::client_log_directives(std::env::var("RUST_LOG").ok()),
        #[cfg(not(feature = "client"))]
        _ => std::env::var("RUST_LOG").unwrap_or_else(|_| "error".to_string()),
    };
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
        .with_writer(std::io::stderr)
        .init();
    match args.first().map(String::as_str) {
        None | Some("serve") => report(serve()),
        Some("mcp") => mcp(),
        Some("call") => call(&args[1..]),
        Some("tools") => tools(&args[1..]),
        Some("kb") => kb(&args[1..]),
        Some("project") => project(&args[1..]),
        Some("config") => config_cmd(&args[1..]),
        Some("enrol") => enrol(&args[1..]),
        Some("backup") => backup_cmd(&args[1..]),
        Some("restore") => restore_cmd(&args[1..]),
        Some("check") => check_cmd(&args[1..]),
        Some("doctor") => doctor_cmd(&args[1..]),
        Some("health") => health_cmd(&args[1..]),
        // An unknown subcommand must not start a hub: a typo would otherwise
        // become a second engine process on the data directory.
        Some(other) => {
            eprintln!("agent-hub: unknown subcommand '{other}'");
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// The usage a help flag asks for, or `None` when there is no help flag.
///
/// `--help` or `-h` anywhere in the arguments answers for the subcommand in
/// front of it, and on its own answers for the binary. A name that is not a
/// subcommand gets no usage from here: a typo is a typo with a help flag on it,
/// and the refusal says so beside the top-level usage.
fn usage_for(args: &[String]) -> Option<&'static str> {
    let is_help = |argument: &String| argument == "--help" || argument == "-h";
    let (command, rest) = args.split_first()?;
    let usage = usage_of(command)?;
    (command == "help" || is_help(command) || rest.iter().any(is_help)).then_some(usage)
}

/// The usage text for one subcommand, or `None` when the name is not one.
fn usage_of(command: &str) -> Option<&'static str> {
    Some(match command {
        "serve" => SERVE_USAGE,
        "mcp" => MCP_USAGE,
        "call" => CALL_USAGE,
        "tools" => TOOLS_USAGE,
        "kb" => KB_USAGE,
        #[cfg(feature = "client")]
        "project" => agent_hub::client::projects::PROJECT_USAGE,
        "config" => CONFIG_USAGE,
        #[cfg(feature = "client")]
        "enrol" => agent_hub::client::enrol::ENROL_USAGE,
        "backup" => BACKUP_USAGE,
        "restore" => RESTORE_USAGE,
        "check" => CHECK_USAGE,
        "doctor" => DOCTOR_USAGE,
        "health" => HEALTH_USAGE,
        "help" | "--help" | "-h" => USAGE,
        _ => return None,
    })
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

/// Copy the whole store into a fresh output directory, or ask a running hub to.
fn backup_cmd(args: &[String]) -> ExitCode {
    if args.iter().any(|argument| argument == "--url") {
        return online_backup_cmd(args);
    }
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

/// Ask the hub named by `--url` to take a backup of its own store.
#[cfg(feature = "client")]
fn online_backup_cmd(args: &[String]) -> ExitCode {
    use agent_hub::client::Failure;

    let mut url = None;
    let mut rest = args.iter();
    while let Some(argument) = rest.next() {
        match argument.as_str() {
            "--url" => match rest.next() {
                Some(value) => url = Some(value.clone()),
                None => {
                    return fail(&Failure::Usage(format!(
                        "backup: --url needs a value\n{BACKUP_USAGE}"
                    )));
                }
            },
            flag @ ("--out" | "--data-dir") => {
                return fail(&Failure::Usage(format!(
                    "backup: --url asks the hub, which chooses where the backup goes, so it \
                     takes no {flag}\n{BACKUP_USAGE}"
                )));
            }
            other => {
                return fail(&Failure::Usage(format!(
                    "backup: unknown option '{other}'\n{BACKUP_USAGE}"
                )));
            }
        }
    }
    let Some(url) = url else {
        return fail(&Failure::Usage(format!(
            "backup: --url needs a value\n{BACKUP_USAGE}"
        )));
    };
    let admin_token = match Config::admin_token_from_env() {
        Ok(Some(token)) => token,
        Ok(None) => {
            return fail(&Failure::Config(
                "backup --url acts as the admin; set HUB_ADMIN_TOKEN, or admin_token under [hub]"
                    .to_string(),
            ));
        }
        Err(err) => return fail(&Failure::Config(err.to_string())),
    };
    let (client, runtime) = match client_runtime() {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    match runtime.block_on(agent_hub::client::backup::request(
        &client,
        &url,
        &admin_token,
    )) {
        Ok(taken) => {
            let manifest = &taken["manifest"];
            println!(
                "backed up {} files, {} bytes, at schema v{} to {} in {} ms",
                manifest["files"],
                manifest["bytes"],
                manifest["schema_version"],
                taken["path"].as_str().unwrap_or_default(),
                taken["duration_ms"]
            );
            ExitCode::SUCCESS
        }
        Err(failure) => fail(&failure),
    }
}

#[cfg(not(feature = "client"))]
fn online_backup_cmd(_args: &[String]) -> ExitCode {
    without_client()
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
    // A project named in the settings is the default for a call that selects a
    // project, matching `kb`, so a hook sets HUB_PROJECT once rather than
    // repeating it on each call. It is not added to a session-store call, which
    // the hub refuses a project on; a tool that takes no project at all ignores
    // the field.
    agent_hub::client::fill_project(tool, &mut arguments, config.project.as_deref());
    emit(runtime.block_on(agent_hub::client::call(&config, tool, arguments)))
}

/// List the hub's tools and their descriptions.
///
/// The listing is read by an agent as often as by a person, and the schema of
/// every tool in the hub is too much of it to take in as one line, so the
/// indented shape is what this prints. `--compact` is the one-line shape a hook
/// pipes onward.
#[cfg(feature = "client")]
fn tools(args: &[String]) -> ExitCode {
    let shape = match agent_hub::client::tools_shape(args) {
        Ok(shape) => shape,
        Err(message) => {
            eprintln!("agent-hub tools: {message}\n{TOOLS_USAGE}");
            return ExitCode::from(2);
        }
    };
    let (config, runtime) = match client_runtime() {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    match runtime.block_on(agent_hub::client::tools(&config)) {
        Ok(listing) => {
            println!("{}", shape.json(&listing));
            ExitCode::SUCCESS
        }
        Err(failure) => fail(&failure),
    }
}

#[cfg(not(feature = "client"))]
fn tools(args: &[String]) -> ExitCode {
    let _ = args;
    without_client()
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
    let guarded = matches!(command, "put" | "revert");
    let prints_a_view = matches!(command, "get" | "list" | "history");
    let syncs = matches!(command, "export" | "import");
    let unused = [
        (options.path.is_some() && syncs, "a page path"),
        (options.dir.is_some() && !syncs, "--dir"),
        (options.force && command != "export", "--force"),
        (options.dry_run && command != "import", "--dry-run"),
        (options.prune && command != "import", "--prune"),
        (options.if_version.is_some() && !guarded, "--if-version"),
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
    if command != "revert"
        && let Some(extra) = &options.version
    {
        return fail(&Failure::Usage(format!(
            "unexpected argument '{extra}'\n{KB_USAGE}"
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
            // The tool's own result is one level of entries with their types,
            // which is what `--json` prints and what the plain form is not.
            if options.json {
                let mut listing = arguments.clone();
                // A listing with no path is the whole base, which the tool reads
                // as an absent path rather than a root one.
                if let Some(page) = &page {
                    listing["path"] = page.clone().into();
                }
                return emit(runtime.block_on(agent_hub::client::call(
                    &config,
                    "brain_list",
                    listing,
                )));
            }
            let pages = match kb_pages(&config, &runtime, &arguments, page.as_deref()) {
                Ok(pages) => pages,
                Err(failure) => return fail(&failure),
            };
            for path in pages {
                println!("{path}");
            }
            ExitCode::SUCCESS
        }
        "delete" => {
            let Some(page) = page else {
                return missing_page();
            };
            arguments["path"] = page.into();
            emit(runtime.block_on(agent_hub::client::call(&config, "brain_delete", arguments)))
        }
        "history" => {
            let Some(page) = page else {
                return missing_page();
            };
            let mut listing = serde_json::json!({"project_id": project, "path": page});
            if options.json {
                return emit(runtime.block_on(agent_hub::client::call(
                    &config,
                    "brain_history",
                    listing,
                )));
            }
            listing["limit"] = agent_hub::limits::KB_HISTORY_ROWS_MAX.into();
            loop {
                let result = match runtime.block_on(agent_hub::client::call(
                    &config,
                    "brain_history",
                    listing.clone(),
                )) {
                    Ok(result) => result,
                    Err(failure) => return fail(&failure),
                };
                for row in result["versions"].as_array().into_iter().flatten() {
                    println!("{}", history_line(row));
                }
                match result["next_before"].as_i64() {
                    Some(before) => listing["before"] = before.into(),
                    None => break,
                }
            }
            ExitCode::SUCCESS
        }
        "forget" => {
            let Some(page) = page else {
                return missing_page();
            };
            match runtime.block_on(agent_hub::client::projects::forget_kb_history(
                &config, &project, &page,
            )) {
                Ok(result) => {
                    let count = result["versions_forgotten"].as_u64().unwrap_or_default();
                    println!(
                        "forgot {count} kept version{} of {}, {} bytes",
                        if count == 1 { "" } else { "s" },
                        result["path"].as_str().unwrap_or(&page),
                        result["bytes_forgotten"]
                    );
                    ExitCode::SUCCESS
                }
                Err(failure) => fail(&failure),
            }
        }
        "revert" => {
            let Some(page) = page else {
                return missing_page();
            };
            let Some(version) = options.version.clone() else {
                return fail(&Failure::Usage(format!(
                    "kb revert needs the version to put back, from kb history\n{KB_USAGE}"
                )));
            };
            let mut revert =
                serde_json::json!({"project_id": project, "path": page, "version": version});
            if let Some(guard) = options.if_version.as_deref() {
                revert["if_version"] = guard.into();
            }
            emit(runtime.block_on(agent_hub::client::call(&config, "brain_revert", revert)))
        }
        "export" | "import" => {
            let Some(dir) = options.dir.as_deref() else {
                return fail(&Failure::Usage(format!(
                    "kb {command} needs the folder: pass --dir <D>\n{KB_USAGE}"
                )));
            };
            if command == "export" {
                kb_export(&config, &runtime, &project, dir, options.force)
            } else {
                kb_import(&config, &runtime, &project, dir, &options)
            }
        }
        other => fail(&Failure::Usage(format!(
            "unknown kb command '{other}'\n{KB_USAGE}"
        ))),
    }
}

/// Write the project knowledge base into a folder and say what was written.
#[cfg(feature = "client")]
fn kb_export(
    config: &ClientConfig,
    runtime: &tokio::runtime::Runtime,
    project: &str,
    dir: &str,
    force: bool,
) -> ExitCode {
    use agent_hub::client::kb_bundle;

    let exported = match runtime.block_on(kb_bundle::export(
        config,
        project,
        std::path::Path::new(dir),
        force,
    )) {
        Ok(exported) => exported,
        Err(failure) => return fail(&failure),
    };
    for page in &exported.skipped {
        eprintln!("agent-hub: skipped {page}: a dot-named path is not read back by kb import");
    }
    for page in &exported.removed {
        println!("removed {page}: the hub no longer has it");
    }
    println!(
        "exported {} pages, {} bytes, from {project} to {dir}",
        exported.pages, exported.bytes
    );
    ExitCode::SUCCESS
}

/// Write a folder's changed pages into the project knowledge base, one line
/// per page that changes or conflicts, and exit 1 when any conflicted.
#[cfg(feature = "client")]
fn kb_import(
    config: &ClientConfig,
    runtime: &tokio::runtime::Runtime,
    project: &str,
    dir: &str,
    options: &KbOptions,
) -> ExitCode {
    use agent_hub::client::kb_bundle::{self, Action};

    let imported = match runtime.block_on(kb_bundle::import(
        config,
        project,
        std::path::Path::new(dir),
        options.dry_run,
        options.prune,
    )) {
        Ok(imported) => imported,
        Err(failure) => return fail(&failure),
    };
    for finding in &imported.warnings {
        eprintln!("agent-hub: lint {}", kb_bundle::describe(finding));
    }
    let would = if options.dry_run { "would " } else { "" };
    let (mut created, mut updated, mut deleted, mut unchanged, mut kept) = (0, 0, 0, 0, 0);
    for change in &imported.changes {
        let path = &change.path;
        match &change.action {
            Action::Unchanged => unchanged += 1,
            Action::Create => {
                created += 1;
                println!("{would}create {path}");
            }
            Action::Update => {
                updated += 1;
                println!("{would}update {path}");
            }
            Action::Delete => {
                deleted += 1;
                println!("{would}delete {path}");
            }
            Action::Kept => {
                kept += 1;
                println!("keep {path}: not in the folder; --prune deletes it");
            }
            Action::HubNewer => {
                kept += 1;
                println!("hub newer {path}: left as is");
            }
            Action::Conflict(why) => println!("conflict {path}: {why}"),
            Action::Gone => println!("delete {path}: already gone"),
            Action::NotAttempted => println!("not attempted {path}"),
        }
    }
    let conflicts = imported.conflicts();
    println!(
        "{}{created} created, {updated} updated, {deleted} deleted, {unchanged} unchanged, {kept} kept, {conflicts} conflicts",
        if options.dry_run {
            "dry run, nothing written: "
        } else {
            ""
        }
    );
    if let Some(first) = imported.failures.first() {
        for failure in &imported.failures {
            failure.report();
        }
        return ExitCode::from(first.exit_code());
    }
    if conflicts > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Create, list, and delete projects from a flag-shaped command line.
///
/// The project routes are the one part of the hub with no MCP tool, so an agent
/// that had to reach them read its token out of the config and handed it to
/// curl. These verbs are that reach, and the same settings, failure object and
/// exit codes the one-shot calls already use.
#[cfg(feature = "client")]
fn project(args: &[String]) -> ExitCode {
    use agent_hub::client::Failure;
    use agent_hub::client::projects::PROJECT_USAGE;

    let Some(command) = args.first().map(String::as_str) else {
        return fail(&Failure::Usage(format!(
            "project needs a command\n{PROJECT_USAGE}"
        )));
    };
    let options = match ProjectOptions::parse(&args[1..]) {
        Ok(options) => options,
        Err(message) => return fail(&Failure::Usage(format!("{message}\n{PROJECT_USAGE}"))),
    };

    // An option a command accepts and then ignores misleads: a --plain on a
    // create reads as a request for output the command never chose to shape.
    let unused = [
        (
            options.id.is_some() && !matches!(command, "create" | "delete"),
            "--id",
        ),
        (options.name.is_some() && command != "create", "--name"),
        (options.plain && command != "list", "--plain"),
    ]
    .into_iter()
    .find_map(|(given, name)| given.then_some(name));
    if let Some(option) = unused {
        return fail(&Failure::Usage(format!(
            "project {command} does not take {option}\n{PROJECT_USAGE}"
        )));
    }

    let (config, runtime) = match client_runtime() {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    match command {
        "create" => {
            let Some(id) = options.id.as_deref() else {
                return fail(&Failure::Usage(format!(
                    "project create needs the id it creates\n{PROJECT_USAGE}"
                )));
            };
            // The hub wants a display name and has no default for one, so the
            // id is the name until the caller says otherwise.
            let name = options.name.as_deref().unwrap_or(id);
            emit(runtime.block_on(agent_hub::client::projects::create(&config, id, name)))
        }
        "list" => {
            let listed = runtime.block_on(agent_hub::client::projects::list(&config));
            if options.plain {
                emit_lines(listed)
            } else {
                emit(listed)
            }
        }
        // With no --id the configured project is the target, which is what a
        // hook that exported HUB_PROJECT means. Whether this caller may delete
        // at all is the hub's answer, passed through as it arrived.
        "delete" => {
            let Some(id) = options.id.or_else(|| config.project.clone()) else {
                return fail(&Failure::Usage(
                    "project delete needs the project it deletes: pass --id <id> or set \
                     HUB_PROJECT"
                        .to_string(),
                ));
            };
            match runtime.block_on(agent_hub::client::projects::delete(&config, &id)) {
                Ok(()) => {
                    println!("deleted {id}");
                    ExitCode::SUCCESS
                }
                Err(failure) => fail(&failure),
            }
        }
        other => fail(&Failure::Usage(format!(
            "unknown project command '{other}'\n{PROJECT_USAGE}"
        ))),
    }
}

/// What a `project` command line carried besides its command.
#[cfg(feature = "client")]
#[derive(Default)]
struct ProjectOptions {
    id: Option<String>,
    name: Option<String>,
    plain: bool,
}

#[cfg(feature = "client")]
impl ProjectOptions {
    /// Parse the closed list of long options, with no positional form: the
    /// project an option names is written out, so a hook's line is legible.
    fn parse(args: &[String]) -> std::result::Result<Self, String> {
        let mut options = Self::default();
        let mut args = args.iter();
        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--id" => options.id = Some(flag_value(&mut args, "--id")?),
                "--name" | "--display-name" => {
                    options.name = Some(flag_value(&mut args, "--name")?)
                }
                "--plain" => options.plain = true,
                flag if flag.starts_with('-') => return Err(format!("unknown flag '{flag}'")),
                extra => return Err(format!("unexpected argument '{extra}'")),
            }
        }
        Ok(options)
    }
}

/// Print one project per line, the way `kb list` prints one page per line.
///
/// The listing is the hub's own body, so the plain form reads the one array it
/// names. The id leads, because that is what every later command names, and the
/// display name follows it; a project with no name yet is its id alone rather
/// than a line with a hole in it. A body that names no array is not an empty
/// listing, so it is reported rather than printed as nothing.
#[cfg(feature = "client")]
fn emit_lines(
    listed: std::result::Result<serde_json::Value, agent_hub::client::Failure>,
) -> ExitCode {
    use agent_hub::client::Failure;

    let listing = match listed {
        Ok(listing) => listing,
        Err(failure) => return fail(&failure),
    };
    let Some(projects) = listing
        .get("projects")
        .and_then(serde_json::Value::as_array)
    else {
        return fail(&Failure::Failed(
            "the hub's project listing named no projects".to_string(),
        ));
    };
    for project in projects {
        let id = project
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        match project
            .get("display_name")
            .and_then(serde_json::Value::as_str)
        {
            Some(name) if !name.is_empty() => println!("{id}  {name}"),
            _ => println!("{id}"),
        }
    }
    ExitCode::SUCCESS
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

/// One line of `kb history`: the version, when, who, what, and whether it is
/// the page as it is now. A delete stored no version, so its column is a dash.
#[cfg(feature = "client")]
fn history_line(row: &serde_json::Value) -> String {
    let text = |key: &str| row[key].as_str().unwrap_or("-").to_string();
    let mut line = format!(
        "{}  {}  {}  {}",
        text("version"),
        text("at"),
        text("actor"),
        text("summary")
    );
    if row["current"].as_bool() == Some(true) {
        line.push_str("  current");
    } else if row["version"].is_string() && row["kept"].as_bool() == Some(false) {
        line.push_str("  not kept");
    }
    line
}

/// What a `kb` command line carried besides its command.
#[cfg(feature = "client")]
#[derive(Default)]
struct KbOptions {
    path: Option<String>,
    /// The second positional argument, which only `kb revert` takes.
    version: Option<String>,
    project: Option<String>,
    if_version: Option<String>,
    file: Option<String>,
    stdin: bool,
    json: bool,
    dir: Option<String>,
    force: bool,
    dry_run: bool,
    prune: bool,
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
                "--dir" => options.dir = Some(flag_value(&mut args, "--dir")?),
                "--force" => options.force = true,
                "--dry-run" => options.dry_run = true,
                "--prune" => options.prune = true,
                "-" => options.stdin = true,
                flag if flag.starts_with('-') => return Err(format!("unknown flag '{flag}'")),
                path if options.path.is_none() => options.path = Some(path.to_string()),
                version if options.version.is_none() => {
                    options.version = Some(version.to_string());
                }
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

/// Every page under `root`, or under the whole base when it names none.
///
/// `brain_list` returns the immediate children of one path and marks each as a
/// file or a directory, so a plain listing is a walk: a page is a line, and a
/// directory is a step into the level below it. Printing a directory would hand
/// a hook a path `kb get` cannot read, and stopping at the first level would
/// hide every page under one. A directory the hub lists as neither a file nor a
/// directory is left out rather than guessed at; `--json` shows what came back.
///
/// Each level is one call in the order the hub lists it, so the pages read in
/// the order the base is written in.
#[cfg(feature = "client")]
fn kb_pages(
    config: &ClientConfig,
    runtime: &tokio::runtime::Runtime,
    arguments: &serde_json::Value,
    root: Option<&str>,
) -> std::result::Result<Vec<String>, agent_hub::client::Failure> {
    let mut listing = arguments.clone();
    if let Some(path) = root {
        listing["path"] = path.into();
    }
    let listed = runtime.block_on(agent_hub::client::call(config, "brain_list", listing))?;
    let mut pages = Vec::new();
    for entry in listed["entries"].as_array().into_iter().flatten() {
        let Some(path) = entry.get("path").and_then(serde_json::Value::as_str) else {
            continue;
        };
        match entry.get("type").and_then(serde_json::Value::as_str) {
            Some("file") => pages.push(path.to_string()),
            Some("dir") => pages.extend(kb_pages(config, runtime, arguments, Some(path))?),
            _ => {}
        }
    }
    Ok(pages)
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
fn kb(_args: &[String]) -> ExitCode {
    without_client()
}

#[cfg(not(feature = "client"))]
fn project(_args: &[String]) -> ExitCode {
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
