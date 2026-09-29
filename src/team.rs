//! `fainder team`: search the team's reviewed conversations through the
//! SmartUp admin API instead of local files.
//!
//! This is a subcommand, not a provider: local search must keep working
//! offline, and a teammate's session cannot be resumed on this machine, so it
//! has no `resume_command`. fainder never talks to the index database; admin
//! owns auth (personal operator keys), scopes and audit.
//!
//! Wire contract (JSON), served by admin. Canonical path is
//! `/api/manage/dev-insights/*`; `/api/dev-insights/*` (used below) is an
//! alias over the same handlers. Every response is wrapped in the Manage
//! API's envelope, `{"success": true, "data": {...}}`; only `data` is shown:
//!
//! ```text
//! GET /api/dev-insights/search?q&mode&dev&harness&machine&repo&project&client&task&pr&since&until&limit
//!   -> { "results": [SearchHit], "total": N }
//! GET /api/dev-insights/sessions/{dev}:{harness}:{native_id}/turns?from&to&key[&tools=1]
//!   -> { "ref": ..., "key": ..., "title": ..., "turns": [Turn] }
//! ```
//!
//! Design: SmartUp-Chile/conversations-context INDEX.md, section 6. Contract
//! source of truth: SmartUp-Chile/admin#231, cross-checked against
//! SmartUp-Chile/plugins `dev-insights/skills/team-search/references/contract.md`
//! (PR #84) — if the two disagree, fix one and note it there.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::config::TeamConfig;
use crate::model::SearchMode;

#[derive(Debug, Clone, Default)]
pub struct TeamSearchOptions {
    pub query: String,
    pub mode: Option<SearchMode>,
    pub dev: Option<String>,
    pub harness: Option<String>,
    pub machine: Option<String>,
    pub repo: Option<String>,
    pub project: Option<String>,
    pub client: Option<String>,
    pub task: Option<String>,
    pub pr: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: usize,
}

#[derive(Debug, Clone)]
pub struct TeamContextOptions {
    pub session_ref: String,
    /// Selects a subagent's turns (the `key` of a search result); the main
    /// session is used when absent.
    pub key: Option<String>,
    pub from_turn: Option<usize>,
    pub to_turn: Option<usize>,
    pub around: Option<usize>,
    pub context: usize,
    pub tools: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Link {
    pub kind: String,
    #[serde(rename = "ref")]
    pub reference: String,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub is_primary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchHit {
    /// `{dev}:{harness}:{native_id}`, the argument `team context` takes.
    #[serde(rename = "ref")]
    pub session: String,
    /// The `session_key`; pass as `--key` to `team context` to read a subagent.
    pub key: String,
    pub dev: String,
    pub harness: String,
    pub native_id: String,
    #[serde(default)]
    pub subagent: bool,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub project_name: Option<String>,
    #[serde(default)]
    pub git_remote: Option<String>,
    #[serde(default)]
    pub git_remotes: Vec<String>,
    #[serde(default)]
    pub machine: Option<String>,
    #[serde(default)]
    pub first: Option<String>,
    #[serde(default)]
    pub last: Option<String>,
    #[serde(default)]
    pub score: f64,
    /// Turn number of the best match, for `team context --around`.
    pub turn: usize,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub snippet: String,
    #[serde(default)]
    pub links: Vec<Link>,
}

impl SearchHit {
    /// The client this session is linked to. The API has no top-level `client`
    /// field; it travels as a `links` entry (`kind == "client"`, at most one
    /// `is_primary`).
    pub fn client(&self) -> Option<&str> {
        self.links
            .iter()
            .find(|link| link.kind == "client" && link.is_primary)
            .or_else(|| self.links.iter().find(|link| link.kind == "client"))
            .map(|link| link.reference.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Turn {
    pub turn: usize,
    pub role: String,
    #[serde(default)]
    pub ts: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    pub text: String,
}

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    data: T,
}

#[derive(Debug, Deserialize)]
struct SearchData {
    results: Vec<SearchHit>,
    #[serde(default)]
    #[allow(dead_code)] // not surfaced yet; kept so the shape matches the contract
    total: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnsResponse {
    #[serde(rename = "ref")]
    pub session_ref: String,
    pub key: String,
    #[serde(default)]
    pub title: Option<String>,
    pub turns: Vec<Turn>,
}

pub struct TeamClient {
    base_url: String,
    api_key: String,
    agent: ureq::Agent,
}

impl TeamClient {
    pub fn from_config(config: &TeamConfig, config_path: &Path) -> Result<Self> {
        let base_url = config.url.clone().ok_or_else(|| {
            anyhow!(
                "team mode is not configured: add [team] url = \"https://admin.smartup.lat\" to {}",
                config_path.display()
            )
        })?;
        let api_key = std::env::var(&config.api_key_env).ok().filter(|k| !k.trim().is_empty()).ok_or_else(|| {
            anyhow!(
                "set {} to your personal operator key (admin, Profile > My Keys)",
                config.api_key_env
            )
        })?;
        Ok(Self::new(base_url, api_key))
    }

    pub fn new(base_url: String, api_key: String) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            agent,
        }
    }

    pub fn search(&self, options: &TeamSearchOptions) -> Result<Vec<SearchHit>> {
        let mut request = self
            .agent
            .get(format!("{}/api/dev-insights/search", self.base_url))
            .header("X-API-Key", &self.api_key)
            .query("q", &options.query)
            .query("limit", options.limit.to_string());
        if let Some(mode) = options.mode {
            request = request.query("mode", mode_param(mode));
        }
        for (key, value) in [
            ("dev", &options.dev),
            ("harness", &options.harness),
            ("machine", &options.machine),
            ("repo", &options.repo),
            ("project", &options.project),
            ("client", &options.client),
            ("task", &options.task),
            ("pr", &options.pr),
            ("since", &options.since),
            ("until", &options.until),
        ] {
            if let Some(value) = value {
                request = request.query(key, value);
            }
        }
        let data: SearchData = read_data(request.call())?;
        Ok(data.results)
    }

    pub fn turns(&self, options: &TeamContextOptions) -> Result<TurnsResponse> {
        validate_session_ref(&options.session_ref)?;
        let (from, to) = turn_window(options)?;
        let mut request = self
            .agent
            .get(format!(
                "{}/api/dev-insights/sessions/{}/turns",
                self.base_url, options.session_ref
            ))
            .header("X-API-Key", &self.api_key);
        if let Some(from) = from {
            request = request.query("from", from.to_string());
        }
        if let Some(to) = to {
            request = request.query("to", to.to_string());
        }
        if let Some(key) = &options.key {
            request = request.query("key", key);
        }
        if options.tools {
            request = request.query("tools", "1");
        }
        read_data(request.call())
    }
}

/// The server's text index splits `src/lib/auth.ts` into tokens, so a phrase or word
/// search never finds an identifier or path whole. When the caller picked no mode and the
/// query is a single word containing `/`, `\`, `.` or `::`, search it as a literal regex.
pub fn resolve_search_mode(query: &str, regex: bool, words: bool) -> (String, SearchMode) {
    if regex {
        return (query.to_string(), SearchMode::Regex);
    }
    if words {
        return (query.to_string(), SearchMode::Words);
    }
    let single_word = query.split_whitespace().count() == 1;
    if single_word && (query.contains(['/', '\\', '.']) || query.contains("::")) {
        return (regex::escape(query), SearchMode::Regex);
    }
    (query.to_string(), SearchMode::Phrase)
}

fn mode_param(mode: SearchMode) -> &'static str {
    match mode {
        SearchMode::Phrase => "phrase",
        SearchMode::Words => "words",
        SearchMode::Regex => "regex",
    }
}

/// Reads the envelope and returns `data`; the server never sends a 2xx with
/// `success: false`, so status alone tells success from error.
fn read_data<T: serde::de::DeserializeOwned>(
    result: std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Result<T> {
    let envelope: Envelope<T> = read_json(result)?;
    Ok(envelope.data)
}

fn read_json<T: serde::de::DeserializeOwned>(
    result: std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Result<T> {
    let mut response = result.context("could not reach the team index")?;
    let status = response.status().as_u16();
    match status {
        200..=299 => response
            .body_mut()
            .read_json()
            .context("unexpected response from the team index"),
        401 => bail!("the team index rejected the operator key (401): check it is valid and not expired"),
        403 => bail!("your operator lacks the devinsights:read scope (403): ask an admin to grant it"),
        404 => bail!("not found (404): the session does not exist or was withdrawn"),
        _ => {
            let body = response.body_mut().read_to_string().unwrap_or_default();
            let body: String = body.chars().take(300).collect();
            bail!("team index returned {status}: {body}")
        }
    }
}

/// `{dev}:{harness}:{native_id}`. Checked before building the URL so a typo
/// can't turn into a different path on the server.
pub fn validate_session_ref(session_ref: &str) -> Result<()> {
    let parts: Vec<&str> = session_ref.split(':').collect();
    let ok = parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
        && !parts.iter().any(|part| *part == "." || *part == "..");
    if ok {
        Ok(())
    } else {
        bail!("session must look like dev:harness:id, for example sat:claude:e52f2113 (got {session_ref:?})")
    }
}

fn turn_window(options: &TeamContextOptions) -> Result<(Option<usize>, Option<usize>)> {
    if let Some(around) = options.around {
        if options.from_turn.is_some() || options.to_turn.is_some() {
            bail!("use either --around or --from-turn/--to-turn, not both");
        }
        return Ok((
            Some(around.saturating_sub(options.context)),
            Some(around + options.context),
        ));
    }
    Ok((options.from_turn, options.to_turn))
}

pub fn print_hits(hits: &[SearchHit]) {
    if hits.is_empty() {
        println!("No team conversations matched.");
        return;
    }
    for (index, hit) in hits.iter().enumerate() {
        let title = hit.title.as_deref().unwrap_or("(untitled)");
        let subagent = if hit.subagent { " [subagent]" } else { "" };
        println!("{}. {}{}", index + 1, title, subagent);
        let mut meta = vec![hit.session.clone()];
        if let Some(client) = hit.client() {
            meta.push(format!("client {client}"));
        }
        if let Some(project) = &hit.project_name {
            meta.push(format!("project {project}"));
        }
        if let Some(last) = &hit.last {
            meta.push(last.chars().take(10).collect());
        }
        println!("   {}", meta.join(" · "));
        let snippet = hit.snippet.split_whitespace().collect::<Vec<_>>().join(" ");
        if !snippet.is_empty() {
            println!(
                "   turn {} {}: {}",
                hit.turn,
                hit.role.as_deref().unwrap_or(""),
                snippet
            );
        }
        let links: Vec<String> = hit
            .links
            .iter()
            .filter(|link| link.kind != "client")
            .map(|link| format!("{} {}", link.kind, link.reference))
            .collect();
        if !links.is_empty() {
            println!("   links: {}", links.join(", "));
        }
        println!(
            "   fainder team context {} --around {}",
            hit.session, hit.turn
        );
    }
}

pub fn print_turns(response: &TurnsResponse) {
    let title = response.title.as_deref().unwrap_or("(untitled)");
    println!("# {} ({})\n", title, response.session_ref);
    for turn in &response.turns {
        let ts = turn.ts.as_deref().unwrap_or("");
        let tool = turn
            .tool_name
            .as_deref()
            .map(|name| format!(" [{name}]"))
            .unwrap_or_default();
        println!("## [{}] {}{} {}\n", turn.turn, turn.role, tool, ts);
        println!("{}\n", turn.text.trim_end());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    /// Serves one canned response and hands back the raw request head.
    fn serve_once(status: &str, body: &str) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                head.push_str(&line);
            }
            stream.write_all(response.as_bytes()).unwrap();
            tx.send(head).unwrap();
        });
        (url, rx)
    }

    const HIT: &str = r#"{"success":true,"data":{"results":[{"ref":"sat:claude:e52f2113","key":"9f2c...","dev":"sat","harness":"claude","native_id":"e52f2113","subagent":false,"title":"Remedición J&A","project_name":"smartorders","git_remote":"SmartUp-Chile/smartorders","git_remotes":["SmartUp-Chile/smartorders"],"machine":"0123456789abcdef","first":"2026-09-17T18:40:00Z","last":"2026-09-17T18:52:00Z","score":0.8,"turn":142,"role":"user","snippet":"el escalation_task_id queda nulo","links":[{"kind":"shapeup_task","ref":"AUTO-03","origin":"detected","is_primary":false},{"kind":"client","ref":"f614a811","origin":"dev","is_primary":true}]}],"total":1}}"#;

    #[test]
    fn search_sends_key_filters_and_parses_hits() {
        let (url, head) = serve_once("200 OK", HIT);
        let client = TeamClient::new(format!("{url}/"), "op_key".into());
        let hits = client
            .search(&TeamSearchOptions {
                query: "escalation task".into(),
                mode: Some(SearchMode::Regex),
                client: Some("pull-a-part".into()),
                task: Some("AUTO-03".into()),
                repo: Some("SmartUp-Chile/smartorders".into()),
                machine: Some("0123456789abcdef".into()),
                limit: 5,
                ..Default::default()
            })
            .unwrap();

        let head = head.recv().unwrap();
        let request_line = head.lines().next().unwrap();
        assert!(request_line.starts_with("GET /api/dev-insights/search?"));
        assert!(request_line.contains("q=escalation"));
        assert!(request_line.contains("mode=regex"));
        assert!(request_line.contains("client=pull-a-part"));
        assert!(request_line.contains("task=AUTO-03"));
        assert!(request_line.contains("repo=SmartUp-Chile"));
        assert!(request_line.contains("machine=0123456789abcdef"));
        assert!(request_line.contains("limit=5"));
        assert!(!request_line.contains("dev="));
        assert!(head.to_ascii_lowercase().contains("x-api-key: op_key"));

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session, "sat:claude:e52f2113");
        assert_eq!(hits[0].key, "9f2c...");
        assert_eq!(hits[0].turn, 142);
        assert_eq!(hits[0].client(), Some("f614a811"));
        assert_eq!(hits[0].git_remotes, vec!["SmartUp-Chile/smartorders".to_string()]);
    }

    #[test]
    fn turns_requests_a_window_around_a_turn_and_can_select_a_subagent() {
        let body = r#"{"success":true,"data":{"ref":"sat:claude:e52f2113","key":"9f2c...","title":"t","turns":[{"turn":140,"role":"user","ts":"2026-09-17T18:40:00Z","tool_name":null,"text":"hola"}]}}"#;
        let (url, head) = serve_once("200 OK", body);
        let client = TeamClient::new(url, "k".into());
        let response = client
            .turns(&TeamContextOptions {
                session_ref: "sat:claude:e52f2113".into(),
                key: Some("9f2c...".into()),
                from_turn: None,
                to_turn: None,
                around: Some(142),
                context: 5,
                tools: true,
            })
            .unwrap();

        let request_line = head.recv().unwrap().lines().next().unwrap().to_string();
        assert!(request_line.starts_with("GET /api/dev-insights/sessions/sat:claude:e52f2113/turns?"));
        assert!(request_line.contains("from=137"));
        assert!(request_line.contains("to=147"));
        assert!(request_line.contains("key=9f2c"));
        assert!(request_line.contains("tools=1"));
        assert_eq!(response.turns[0].text, "hola");
    }

    #[test]
    fn missing_scope_explains_itself() {
        let (url, _head) = serve_once("403 Forbidden", "{}");
        let client = TeamClient::new(url, "k".into());
        let err = client
            .search(&TeamSearchOptions {
                query: "x".into(),
                limit: 1,
                ..Default::default()
            })
            .unwrap_err();
        assert!(err.to_string().contains("devinsights:read"));
    }

    #[test]
    fn session_ref_is_validated_before_any_request() {
        assert!(validate_session_ref("sat:claude:e52f2113").is_ok());
        assert!(validate_session_ref("max:codex:019dec17-aa").is_ok());
        for bad in ["claude:e52f2113", "sat:claude:../x", "sat:claude:a/b", "sat::x", "..:claude:x"] {
            assert!(validate_session_ref(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn around_and_explicit_range_are_exclusive() {
        let options = TeamContextOptions {
            session_ref: "a:b:c".into(),
            key: None,
            from_turn: Some(1),
            to_turn: None,
            around: Some(10),
            context: 3,
            tools: false,
        };
        assert!(turn_window(&options).is_err());
        let options = TeamContextOptions {
            from_turn: None,
            around: Some(2),
            ..options
        };
        assert_eq!(turn_window(&options).unwrap(), (Some(0), Some(5)));
    }

    #[test]
    fn client_falls_back_to_any_client_link_when_none_is_primary() {
        let mut hit: SearchHit = {
            let data: serde_json::Value = serde_json::from_str(HIT).unwrap();
            serde_json::from_value(data["data"]["results"][0].clone()).unwrap()
        };
        hit.links.iter_mut().for_each(|l| l.is_primary = false);
        assert_eq!(hit.client(), Some("f614a811"));
        hit.links.retain(|l| l.kind != "client");
        assert_eq!(hit.client(), None);
    }

    #[test]
    fn identifiers_and_paths_default_to_literal_regex() {
        let (q, mode) = resolve_search_mode("src/lib/a.ts", false, false);
        assert_eq!(q, r"src/lib/a\.ts");
        assert!(matches!(mode, SearchMode::Regex));
        let (q, mode) = resolve_search_mode("Foo::bar", false, false);
        assert_eq!(q, "Foo::bar");
        assert!(matches!(mode, SearchMode::Regex));
    }

    #[test]
    fn plain_queries_and_explicit_flags_keep_their_mode() {
        assert!(matches!(resolve_search_mode("rollback", false, false).1, SearchMode::Phrase));
        assert!(matches!(resolve_search_mode("see a.ts now", false, false).1, SearchMode::Phrase));
        assert!(matches!(resolve_search_mode("a/b.ts", false, true).1, SearchMode::Words));
        let (q, mode) = resolve_search_mode("a.*b", true, false);
        assert_eq!(q, "a.*b");
        assert!(matches!(mode, SearchMode::Regex));
    }
}
