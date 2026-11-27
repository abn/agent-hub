//! A phone for the browser checks: it pairs with a hub from a pairing URI, the
//! way the app does through Tendril, and prints what the operator compares.
//!
//! `tendril-phone <pairing URI> <name>` prints `{"device_id", "check_string"}`
//! and exits, leaving the phone pending on the hub. It keeps no state.
//!
//! `tendril-phone <pairing URI> <name> --stay` prints the same, then keeps
//! dialling the hub every half second, as the app does while it waits for the
//! operator, until it is stopped or a minute has passed. Once the operator
//! confirms it, each dial opens a session, which is the phone checking in.

#[cfg(unix)]
mod phone {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use tendril_core::identity::EndpointIdentity;
    use tendril_core::pairing::{PairOutcome, PairingUri};
    use tendril_core::roster::Roster;
    use tendril_runtime::providers::hints::{HintDialerConfig, HintDialers};
    use tendril_runtime::{
        ConnectOpts, PairPolicyOptions, RosterAccess, SessionDriver,
        drive_pair_initiator_with_policy,
    };

    const STAY: Duration = Duration::from_secs(60);
    const DIAL_EVERY: Duration = Duration::from_millis(500);

    pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
        let mut args = std::env::args().skip(1);
        let (Some(uri), Some(name)) = (args.next(), args.next()) else {
            return Err("usage: tendril-phone <pairing URI> <name> [--stay]".into());
        };
        let stay = args.next().as_deref() == Some("--stay");
        let parsed = PairingUri::parse(&uri)?;
        let identity = Arc::new(EndpointIdentity::generate()?);
        let roster = RosterAccess::memory(Roster::new());
        let dialers = Arc::new(HintDialers::new(HintDialerConfig::default()));
        let mut opts =
            PairPolicyOptions::new(name, "android", parsed.code.clone(), Arc::clone(&dialers));
        opts.uri = Some(parsed);
        let paired = drive_pair_initiator_with_policy(Arc::clone(&identity), &roster, opts)
            .await
            .map_err(|err| err.to_string())?;
        let PairOutcome::InitiatorPending {
            pairing_sas,
            host_id,
            ..
        } = paired.drive.outcome
        else {
            return Err("the hub did not take the claim".into());
        };
        println!(
            "{}",
            serde_json::json!({ "device_id": identity.device_id(), "check_string": pairing_sas })
        );
        if !stay {
            return Ok(());
        }
        let until = Instant::now() + STAY;
        while Instant::now() < until {
            // A pending phone is refused in the handshake. A confirmed one is
            // admitted, and the session is held for a moment before the next.
            if let Ok(connected) = SessionDriver::connect_with_policy_and_store(
                Arc::clone(&identity),
                &host_id,
                &roster,
                ConnectOpts::new(Arc::clone(&dialers)),
            )
            .await
            {
                tokio::time::sleep(DIAL_EVERY).await;
                drop(connected);
            }
            tokio::time::sleep(DIAL_EVERY).await;
        }
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
