//! The knowledge base as MCP resources.
//!
//! A real hub serves on an ephemeral port and the built binary runs as the
//! stdio proxy with an agent token, so every listing and read here crosses the
//! proxy and the hub's streamable HTTP transport and is held to the agent's
//! access. One test runs embedded stdio as the local admin, for the transport
//! that never leaves the process.

use std::collections::HashSet;

use serde_json::{Value, json};

mod common;

use common::hub::{ADMIN_TOKEN, AGENT, Hub, PROJECT};
use common::stdio::{StdioClient, structured};
use common::temp::TempDir;
use common::wire;

/// The JSON-RPC error code `resources/read` answers an unknown URI with.
const RESOURCE_NOT_FOUND: i64 = -32002;
const INVALID_PARAMS: i64 = -32602;
/// What a hub `forbidden` travels as.
const INVALID_REQUEST: i64 = -32600;

fn proxy(hub: &Hub) -> StdioClient {
    let mut client = StdioClient::spawn(
        &["mcp"],
        &[
            ("HUB_URL", hub.url()),
            ("HUB_TOKEN", hub.agent_token.clone()),
            ("HUB_CONFIG", hub.config_path().display().to_string()),
        ],
    );
    client.initialize();
    client
}

/// Write a page as the agent, through the project store tool.
fn put_page(client: &mut StdioClient, project: &str, path: &str, content: &str) {
    let put = client.call_tool(
        "brain_put",
        json!({"store": "project", "project_id": project, "path": path, "content": content}),
    );
    assert_eq!(structured(&put)["ok"], true, "the page was written: {put}");
}

/// Write a page as the admin, over REST.
fn admin_put_page(hub: &Hub, project: &str, path: &str, content: &str) {
    let written = wire::rest(
        hub.port,
        "PUT",
        &format!("/api/v1/projects/{project}/kb/pages/{path}"),
        Some(ADMIN_TOKEN),
        Some(&json!({"content": content}).to_string()),
    );
    assert_eq!(written.status, 200, "the page was written: {}", written.raw);
}

/// Every URI one listing page names, and its cursor.
fn list_page(client: &mut StdioClient, cursor: Option<&str>) -> (Vec<String>, Option<String>) {
    let params = match cursor {
        Some(cursor) => json!({"cursor": cursor}),
        None => json!({}),
    };
    let listed = client.call("resources/list", params);
    let uris = listed["result"]["resources"]
        .as_array()
        .unwrap_or_else(|| panic!("resources/list returns an array: {listed}"))
        .iter()
        .map(|resource| {
            resource["uri"]
                .as_str()
                .expect("every resource has a uri")
                .to_string()
        })
        .collect();
    let next = listed["result"]["nextCursor"].as_str().map(str::to_string);
    (uris, next)
}

/// Every URI across every page of the listing.
fn list_all(client: &mut StdioClient) -> Vec<String> {
    let mut all = Vec::new();
    let mut cursor = None;
    loop {
        let (uris, next) = list_page(client, cursor.as_deref());
        all.extend(uris);
        match next {
            Some(next) => cursor = Some(next),
            None => return all,
        }
    }
}

fn read_text(client: &mut StdioClient, uri: &str) -> String {
    let read = client.call("resources/read", json!({"uri": uri}));
    let content = read["result"]["contents"]
        .as_array()
        .and_then(|contents| contents.first())
        .unwrap_or_else(|| panic!("resources/read returns contents for {uri}: {read}"));
    assert_eq!(content["uri"], uri, "the contents name the URI read");
    content["text"]
        .as_str()
        .unwrap_or_else(|| panic!("the contents are text: {read}"))
        .to_string()
}

fn read_error(client: &mut StdioClient, uri: &str) -> Value {
    let read = client.call("resources/read", json!({"uri": uri}));
    read.get("error")
        .cloned()
        .unwrap_or_else(|| panic!("reading {uri} is refused: {read}"))
}

#[test]
fn a_page_is_listed_and_read_through_the_proxy() {
    let hub = Hub::start("resources-page");
    let mut client = proxy(&hub);
    put_page(
        &mut client,
        PROJECT,
        "/fs/svc/caddy notes.md",
        "# Caddy\n\nThe reverse proxy.\n",
    );

    let listed = client.call("resources/list", json!({}));
    let resources = listed["result"]["resources"]
        .as_array()
        .unwrap_or_else(|| panic!("resources/list returns an array: {listed}"));
    let uri = "agenthub://kb/homelab/svc/caddy%20notes.md";
    let page = resources
        .iter()
        .find(|resource| resource["uri"] == uri)
        .unwrap_or_else(|| panic!("the page is listed: {listed}"));
    assert_eq!(page["name"], "svc/caddy notes.md", "{page}");
    assert_eq!(page["title"], "Homelab: svc/caddy notes.md", "{page}");
    assert_eq!(page["mimeType"], "text/markdown", "{page}");
    assert_eq!(page["size"], 28, "{page}");
    assert!(
        listed["result"]["nextCursor"].is_null(),
        "one page holds everything: {listed}"
    );

    assert_eq!(
        read_text(&mut client, uri),
        "# Caddy\n\nThe reverse proxy.\n"
    );

    // A spelling of the same path that the store canonicalises reads the page.
    assert_eq!(
        read_text(&mut client, "agenthub://kb/homelab/svc/./caddy%20notes.md"),
        "# Caddy\n\nThe reverse proxy.\n"
    );

    let templates = client.call("resources/templates/list", json!({}));
    let template = &templates["result"]["resourceTemplates"][0];
    assert_eq!(
        template["uriTemplate"], "agenthub://kb/{project_id}/{+path}",
        "the page template comes through the proxy: {templates}"
    );
    assert!(
        template.get("mimeType").is_none(),
        "a page's media type follows its name, so the template names none: {template}"
    );
}

#[test]
fn the_guide_and_the_bootstrap_are_read_through_the_proxy() {
    let hub = Hub::start("resources-guide");
    let mut client = proxy(&hub);

    let (uris, _) = list_page(&mut client, None);
    for expected in ["agenthub://skill", "skill://agent-hub/SKILL.md"] {
        assert!(
            uris.iter().any(|uri| uri == expected),
            "{expected} is listed: {uris:?}"
        );
    }
    let bootstrap = read_text(&mut client, "agenthub://skill");
    assert!(
        bootstrap.contains(&hub.url()),
        "the bootstrap names the hub's own address"
    );
    let guide = read_text(&mut client, "skill://agent-hub/SKILL.md");
    assert!(guide.starts_with("---\n"), "the guide is the skill itself");
}

#[test]
fn a_confidential_project_is_hidden_until_the_agent_holds_a_grant() {
    let hub = Hub::start("resources-confidential");
    let created = wire::rest(
        hub.port,
        "POST",
        "/api/v1/projects",
        Some(ADMIN_TOKEN),
        Some(r#"{"id":"vault","display_name":"Vault","confidential":true}"#),
    );
    assert_eq!(created.status, 200, "{}", created.raw);
    admin_put_page(&hub, "vault", "keys.md", "where the keys are");

    let mut client = proxy(&hub);
    put_page(&mut client, PROJECT, "/fs/open.md", "anyone may read this");
    let uri = "agenthub://kb/vault/keys.md";

    let before = list_all(&mut client);
    assert!(
        before
            .iter()
            .any(|listed| listed == "agenthub://kb/homelab/open.md"),
        "an ordinary project is listed: {before:?}"
    );
    assert!(
        !before
            .iter()
            .any(|listed| listed.starts_with("agenthub://kb/vault/")),
        "a confidential project without a grant is not listed: {before:?}"
    );
    let refused = read_error(&mut client, uri);
    assert_eq!(refused["code"], INVALID_REQUEST, "{refused}");
    assert_eq!(refused["data"]["error"]["code"], "forbidden", "{refused}");
    // A project that does not exist is refused the same way, so the refusal
    // says nothing about whether the vault is there.
    let missing = read_error(&mut client, "agenthub://kb/nowhere/keys.md");
    assert_eq!(missing["data"]["error"], refused["data"]["error"]);

    let granted = wire::rest(
        hub.port,
        "POST",
        &format!("/api/v1/agents/{AGENT}/grants"),
        Some(ADMIN_TOKEN),
        Some(r#"{"project_id":"vault"}"#),
    );
    assert_eq!(granted.status, 201, "{}", granted.raw);

    let after = list_all(&mut client);
    assert!(
        after.iter().any(|listed| listed == uri),
        "a granted confidential project is listed: {after:?}"
    );
    assert_eq!(read_text(&mut client, uri), "where the keys are");
}

#[test]
fn an_unknown_or_malformed_uri_is_refused() {
    let hub = Hub::start("resources-unknown");
    let mut client = proxy(&hub);

    let unknown = read_error(&mut client, "agenthub://nothing");
    assert_eq!(unknown["code"], RESOURCE_NOT_FOUND, "{unknown}");

    let absent = read_error(&mut client, "agenthub://kb/homelab/absent.md");
    assert_eq!(absent["code"], RESOURCE_NOT_FOUND, "{absent}");
    assert_eq!(absent["data"]["error"]["code"], "not_found", "{absent}");

    for malformed in ["agenthub://kb/homelab", "agenthub://kb/homelab/"] {
        let refused = read_error(&mut client, malformed);
        assert_eq!(refused["code"], INVALID_PARAMS, "{malformed}: {refused}");
    }

    let listed = client.call("resources/list", json!({"cursor": "not-a-cursor"}));
    assert_eq!(listed["error"]["code"], INVALID_PARAMS, "{listed}");
}

#[test]
fn a_knowledge_base_that_cannot_be_opened_is_left_out_of_the_listing() {
    let hub = Hub::start("resources-broken");
    let created = wire::rest(
        hub.port,
        "POST",
        "/api/v1/projects",
        Some(ADMIN_TOKEN),
        Some(r#"{"id":"archive","display_name":"Archive"}"#),
    );
    assert_eq!(created.status, 200, "{}", created.raw);
    admin_put_page(&hub, "archive", "old.md", "an old page");
    let mut client = proxy(&hub);
    put_page(&mut client, PROJECT, "/fs/open.md", "listed");

    // A directory where the store file was: it exists, and the engine cannot
    // open it.
    let store = hub.data_dir().join("kb").join("archive");
    for entry in std::fs::read_dir(&store).expect("the archive store directory") {
        let path = entry.expect("a store directory entry").path();
        std::fs::remove_file(&path).expect("remove a store file");
    }
    std::fs::create_dir(store.join("kb.db")).expect("put a directory in its place");

    let listed = list_all(&mut client);
    assert!(
        listed.contains(&"agenthub://kb/homelab/open.md".to_string()),
        "the other projects are still listed: {listed:?}"
    );
    assert!(
        !listed
            .iter()
            .any(|uri| uri.starts_with("agenthub://kb/archive/")),
        "the store that cannot open is left out: {listed:?}"
    );
}

#[test]
fn a_cursor_inside_one_project_resumes_into_the_next() {
    let hub = Hub::start("resources-two-projects");
    let created = wire::rest(
        hub.port,
        "POST",
        "/api/v1/projects",
        Some(ADMIN_TOKEN),
        Some(r#"{"id":"archive","display_name":"Archive"}"#),
    );
    assert_eq!(created.status, 200, "{}", created.raw);
    let mut client = proxy(&hub);
    // `archive` sorts before `homelab`, and holds more than the first page.
    let archived = 110;
    for at in 0..archived {
        put_page(&mut client, "archive", &format!("/fs/{at:03}.md"), "old");
    }
    let current = ["/fs/a.md", "/fs/b/c.md"];
    for path in current {
        put_page(&mut client, PROJECT, path, "new");
    }

    let (first, cursor) = list_page(&mut client, None);
    let cursor = cursor.expect("a full page carries a cursor");
    assert!(
        cursor.starts_with("agenthub://kb/archive/"),
        "the first page ends inside the first project: {cursor}"
    );
    let (second, end) = list_page(&mut client, Some(&cursor));
    assert!(end.is_none(), "the second page is the last");
    let kb: Vec<&String> = first
        .iter()
        .chain(&second)
        .filter(|uri| uri.starts_with("agenthub://kb/"))
        .collect();
    assert_eq!(
        kb.len(),
        archived + current.len(),
        "every page once: {kb:?}"
    );
    assert!(
        kb.windows(2).all(|pair| pair[0] < pair[1]),
        "pages are listed in project and path order"
    );
    assert_eq!(
        second[second.len() - 2..],
        [
            "agenthub://kb/homelab/a.md".to_string(),
            "agenthub://kb/homelab/b/c.md".to_string()
        ],
        "the next project follows the first: {second:?}"
    );
}

#[test]
fn a_long_listing_is_paged_with_a_cursor() {
    let hub = Hub::start("resources-pages");
    let mut client = proxy(&hub);
    let pages = 120;
    for at in 0..pages {
        put_page(
            &mut client,
            PROJECT,
            &format!("/fs/notes/{at:03}.md"),
            "note",
        );
    }
    // Pages either side of the directory, one named like it, so the walk
    // crosses directories in path order.
    let around = ["/fs/a.md", "/fs/notes.md", "/fs/zz/last.md"];
    for path in around {
        put_page(&mut client, PROJECT, path, "note");
    }
    let pages = pages + around.len();

    let (first, cursor) = list_page(&mut client, None);
    assert_eq!(first.len(), 100, "a page holds at most 100 resources");
    let cursor = cursor.expect("a full page carries a cursor");
    assert_eq!(
        &cursor,
        first.last().expect("a full page"),
        "the cursor is the last URI listed"
    );

    let (second, end) = list_page(&mut client, Some(&cursor));
    assert!(end.is_none(), "the second page is the last");
    let kb: Vec<&String> = first
        .iter()
        .chain(&second)
        .filter(|uri| uri.starts_with("agenthub://kb/"))
        .collect();
    assert_eq!(kb.len(), pages, "every page is listed once: {kb:?}");
    let distinct: HashSet<&&String> = kb.iter().collect();
    assert_eq!(distinct.len(), pages, "no page is listed twice");
    assert!(
        kb.windows(2).all(|pair| pair[0] < pair[1]),
        "pages are listed in path order"
    );
}

#[test]
fn embedded_stdio_lists_and_reads_pages_as_the_local_admin() {
    let data_dir = TempDir::new("resources-embedded");
    common::seed::seed_project(data_dir.path(), "proj");
    let mut client = StdioClient::mcp(data_dir.path(), &[]);
    client.initialize();
    put_page(&mut client, "proj", "/fs/runbook.md", "restart the service");

    let all = list_all(&mut client);
    assert!(
        all.iter().any(|uri| uri == "agenthub://kb/proj/runbook.md"),
        "{all:?}"
    );
    assert_eq!(
        read_text(&mut client, "agenthub://kb/proj/runbook.md"),
        "restart the service"
    );
    let absent = read_error(&mut client, "agenthub://kb/missing/runbook.md");
    assert_eq!(absent["data"]["error"]["code"], "not_found", "{absent}");
}
