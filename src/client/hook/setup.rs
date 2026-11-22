//! `agent-hub setup`: install the hub's plugin into a harness.

/// What `agent-hub setup` takes on its command line.
pub const SETUP_USAGE: &str = "\
usage:
  agent-hub setup antigravity [--workspace DIR]
                                 install the hub plugin into Antigravity

  --workspace DIR  install into DIR/.agents/plugins instead of the user's
                   Antigravity plugin directory

The plugin runs the hook commands, so agent-hub has to be on PATH, and it
reaches the hub through config.toml, which agent-hub enrol writes.
";

/// Run `agent-hub setup` and return its exit code.
pub fn run_setup(args: &[String]) -> u8 {
    let Some((target, rest)) = args.split_first() else {
        eprintln!("agent-hub setup: a target is needed\n{SETUP_USAGE}");
        return 2;
    };
    if target != "antigravity" {
        eprintln!("agent-hub setup: unknown target '{target}'\n{SETUP_USAGE}");
        return 2;
    }
    let mut rest = rest.iter();
    while let Some(argument) = rest.next() {
        match argument.as_str() {
            "--workspace" if rest.next().is_some() => {}
            other => {
                eprintln!("agent-hub setup: unexpected argument '{other}'\n{SETUP_USAGE}");
                return 2;
            }
        }
    }
    eprintln!("agent-hub setup: antigravity is not yet available in this build");
    1
}
