//! A phone for the browser checks: it pairs with a hub from a pairing URI, the
//! way the app does through Tendril, and prints what the operator compares.
//!
//! `tendril-phone <pairing URI> <name>` prints `{"device_id", "check_string"}`
//! and exits, leaving the phone pending on the hub. It keeps no state.

#[cfg(unix)]
mod phone {
    use std::sync::Arc;

    use tendril_core::identity::EndpointIdentity;
    use tendril_core::pairing::{PairOutcome, PairingUri};
    use tendril_core::roster::Roster;
    use tendril_runtime::providers::hints::{HintDialerConfig, HintDialers};
    use tendril_runtime::{PairPolicyOptions, RosterAccess, drive_pair_initiator_with_policy};

    pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
        let mut args = std::env::args().skip(1);
        let (Some(uri), Some(name)) = (args.next(), args.next()) else {
            return Err("usage: tendril-phone <pairing URI> <name>".into());
        };
        let parsed = PairingUri::parse(&uri)?;
        let identity = Arc::new(EndpointIdentity::generate()?);
        let roster = RosterAccess::memory(Roster::new());
        let mut opts = PairPolicyOptions::new(
            name,
            "android",
            parsed.code.clone(),
            Arc::new(HintDialers::new(HintDialerConfig::default())),
        );
        opts.uri = Some(parsed);
        let paired = drive_pair_initiator_with_policy(Arc::clone(&identity), &roster, opts)
            .await
            .map_err(|err| err.to_string())?;
        let PairOutcome::InitiatorPending { pairing_sas, .. } = paired.drive.outcome else {
            return Err("the hub did not take the claim".into());
        };
        println!(
            "{}",
            serde_json::json!({ "device_id": identity.device_id(), "check_string": pairing_sas })
        );
        Ok(())
    }
}

#[cfg(unix)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    phone::run().await
}

/// Phones are unix-only until Tendril builds on Windows.
#[cfg(not(unix))]
fn main() {
    eprintln!("tendril-phone: phones are not available on Windows yet");
    std::process::exit(1);
}
