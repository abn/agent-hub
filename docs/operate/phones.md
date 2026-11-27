---
type: Guide
title: Phones
description: Pair the operator's phones through Tendril, confirm them, and unpair one
tags: [operator, phones, tendril, pairing]
status: stable
---

# Phones

By the end your phone will reach the hub as you, from the home LAN, over a
tailnet or from anywhere else, with no certificate on the hub and no token on
the phone. The decision behind it is
[ADR 0041](/adr/0041-phones-pair-and-connect-through-tendril.md).

Phones are not available on Windows hubs yet.

## Before you start

| Thing | Why |
|---|---|
| The Agent Hub app on the phone | it scans the code and holds the phone's Tendril key |
| The web app, with the admin token when the hub has one | pairing and unpairing are the operator's controls |

## Add a phone

1. Open **Settings > Phones** and press **Add a phone**. The hub shows a QR
   code, the code in letters, the LAN or tailnet address to type beside it
   when it has one, and when the code runs out. The code is always three
   groups, `TND-XXXX-XXXX-XXXX`. It works once and for 15 minutes; a new code
   or **Close** withdraws it.
2. Scan the QR code with the app. Without a camera, type the code and the
   address instead.
3. Both screens show a check string. Compare them. If they are the same, press
   **They match, confirm**. If they differ, press **They differ, refuse**:
   something stands between the hub and the phone.
4. After the confirm the screen waits for the phone to pick it up, which
   usually takes a few seconds, and then says the phone is paired. If two
   minutes pass with no word from the phone, open Agent Hub on it.

Until you confirm, the phone gets nothing from the hub.

The screen follows the hub while it is open: it moves from the code to the
check string when the phone arrives, and on to "paired" when the phone checks
in, with no reload. While a phone waits, the tab's title starts with "(1)".
If the browser allows notifications, a phone that arrives while the screen is
open raises one; the page asks for that permission only when you press
**Add a phone**. A code that runs out before a phone uses it says so, with
**New code** to make another.

The first **Add a phone** starts the hub's Tendril host. While a phone is
paired, the hub listens for phones on the address in `HUB_PHONES_LISTEN` (the
bind address, port 47319, by default) and, unless `HUB_PHONES_RELAY=off`,
through Tendril's public relay. Tendril picks the route each time the phone
connects; there is nothing to choose. A hub bound to loopback offers no LAN
address, so its phones connect through the relay.

The relay is the one place the hub uses a service it does not run. It carries
the phone's end-to-end encrypted session and sees no content, only that two
keys talk, and nothing reaches it before a phone is added. Set
`HUB_PHONES_RELAY=off` to keep phones to the LAN and a tailnet.

In a container, the phone listener is `0.0.0.0:47319` and the image, the
compose file and the Quadlet unit publish port 47319. Publish it to the
interfaces your phones use, or not at all if they only use the relay.

## What a phone may do

A confirmed phone is you, as the web app is, for what the app needs: it reads,
answers, approves, prunes and approves joining machines. It cannot create
members, issue or revoke tokens, change grants, pair or unpair phones, change
notify targets, rename a project, make one public, delete one, take a backup,
or make a share link or viewer pass, a link that would keep working after the
phone is unpaired. Those stay in the web app. A phone approves a joining
machine as it asked: it may grant the projects the machine asked for, or some
of them, and nothing else, and it cannot rename the machine or share. Do those
in the web app. A machine a phone approves keeps its token after the phone is
unpaired; revoke it in **Members and tokens**.

Pairing, confirming and unpairing take requests only from the hub's own web
app, so a page on another site open in your browser cannot pair a phone.

Settings > Phones works when you open the web app at an address the hub owns:
an IP address, `localhost`, or the host of `HUB_PUBLIC_URL`. Any other name
could have been pointed at the hub by someone else's DNS, so the hub refuses
the phone routes under it with `host_not_allowed`. If you reach the hub by a
name, such as a tailnet name or one behind a reverse proxy, set
`HUB_PUBLIC_URL` to that address.

## Unpair a phone

Press **Unpair** beside the phone in **Settings > Phones**. A connected phone
stops at once and shows "This phone was unpaired". A phone that is offline is
refused on its next try. Pairing it again needs a new code.

## Backups

A backup does not carry the hub's Tendril key and roster (`tendril/` in the
data directory). Restore a backup on the node that made it, with `tendril/`
left in place, and phones keep working. Restore it anywhere else and the hub
has a new key no phone knows: unpair every phone in **Settings > Phones** and
pair each again.

## Turn phones off

Set `HUB_PHONES=off`. Add a phone is refused and no Tendril host starts. See
[Configuration](/reference/configuration.md) for every phone setting.
