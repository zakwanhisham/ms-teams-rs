//! `teams-cli` — headless Teams client used for protocol exploration (Phase 2),
//! integration tests, and debugging without the GUI.
//!
//! Current commands:
//! - `teams-cli auth login|status|authz|whoami|logout` (Phase 2A spike harness)
//!
//! Planned later (see `TODO.md`): `conversations`, `history`, `watch`, `send`, `call`.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use teams_auth::{
    AUDIENCE_CSA, AUDIENCE_SPACES, AUDIENCE_SUBSTRATE, AuthzClient, AuthzSession, DeviceCodeClient,
    FOCI_CLIENT_ID, SCOPE_CSA, SCOPE_SPACES, SCOPE_SUBSTRATE, TokenCache, decode_claims,
    identity_from_claims,
};

/// Unofficial Microsoft Teams client for Linux (headless).
#[derive(Parser, Debug)]
#[command(name = "teams-cli", version, about, long_about = None)]
struct Cli {
    /// Increase log verbosity (-v = debug, -vv = trace).
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Authentication (Phase 2A spike): login, cache and authz exchange
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },
    /// List chat/channel conversations via the chat service (Phase 2A/4)
    Conversations {
        /// Page size for the conversations page
        #[arg(long, default_value_t = 5)]
        page_size: u32,
        /// Write the response (re-serialized, round-trip lossless) to this path for
        /// fixture generation — only use paths under the gitignored docs/traces/
        #[arg(long)]
        raw_out: Option<std::path::PathBuf>,
    },
    /// List the messages of one conversation (Phase 4 read path)
    History {
        /// Conversation id, exactly as printed by `teams-cli conversations`
        #[arg(long)]
        conv: String,
        /// Page size for the messages page
        #[arg(long, default_value_t = 10)]
        page_size: u32,
        /// Fetch one thread instead of the conversation's messages: root message id
        #[arg(long)]
        root: Option<String>,
        /// Fetch a single message by id instead of a page
        #[arg(long)]
        message: Option<String>,
        /// Write the response JSON to this path (contains message data — only use paths
        /// under the gitignored docs/traces/, never commit the output)
        #[arg(long)]
        raw_out: Option<std::path::PathBuf>,
    },
    /// Team/channel roster via CSA (Phase 4 read path)
    Roster {
        /// Write the response JSON to this path (contains team/channel names — only use
        /// paths under the gitignored docs/traces/, never commit the output)
        #[arg(long)]
        raw_out: Option<std::path::PathBuf>,
    },
    /// Search the directory for people via substrate (Phase 4 read path)
    Search {
        /// Query string (name, email or job title)
        query: String,
        /// Maximum number of suggestions to request
        #[arg(long, default_value_t = 10)]
        limit: u32,
        /// Write the response JSON to this path (contains directory data — only use paths
        /// under the gitignored docs/traces/, never commit the output)
        #[arg(long)]
        raw_out: Option<std::path::PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum AuthAction {
    /// Device-code sign-in: cache the token, then run the authz exchange
    Login {
        /// Tenant path segment: `common`, `organizations`, `consumers` or a tenant id
        #[arg(long, default_value = "common")]
        tenant: String,
        /// Scope for the initial token (defaults to the verified spaces.skype scope)
        #[arg(long)]
        scope: Option<String>,
    },
    /// Show cached identity and token expiry (never prints token values)
    Status {
        /// Also list every service name from the authz `regionGtms` map
        #[arg(short, long)]
        all_services: bool,
    },
    /// (Re)run the authz exchange with the cached spaces token
    Authz,
    /// Mint a bearer for another audience from the family refresh token
    Mint {
        /// Scope to redeem (`<resource>/.default` form); defaults to the CSA roster audience
        #[arg(long)]
        scope: Option<String>,
        /// Cache label for the minted token; defaults to `chatsvcagg`
        #[arg(long)]
        audience: Option<String>,
        /// Tenant path segment used for the redemption
        #[arg(long, default_value = "common")]
        tenant: String,
    },
    /// Decode (unverified) claims of the cached spaces token
    Whoami,
    /// Delete the local token cache
    Logout,
}

/// `teams-cli … | head` closes our stdout early. Rust ignores SIGPIPE, so `println!`
/// reports `EPIPE` by panicking — which would make an ordinary pipeline print a
/// backtrace and exit 101. Turn that *one* case into a quiet exit; real panics keep
/// the default reporting.
fn install_broken_pipe_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if is_broken_pipe(info.payload()) {
            // The reader chose to stop reading; nothing left to do. Exit 0 so `set -e`
            // pipelines don't see a failure the reader caused.
            std::process::exit(0);
        }
        default_hook(info);
    }));
}

/// True when a panic payload is the stdio "failed printing … Broken pipe" message.
fn is_broken_pipe(payload: &(dyn std::any::Any + Send)) -> bool {
    fn mentions_broken_pipe(message: &str) -> bool {
        message.contains("Broken pipe")
    }
    payload.downcast_ref::<String>().is_some_and(|message| mentions_broken_pipe(message))
        || payload
            .downcast_ref::<&'static str>()
            .is_some_and(|message| mentions_broken_pipe(message))
}

#[tokio::main]
async fn main() {
    install_broken_pipe_hook();

    let cli = Cli::parse();

    let level = match cli.verbose {
        0 => "warn",
        1 => "debug",
        _ => "trace",
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level)),
        )
        .with_target(false)
        .init();

    if let Err(err) = run(&cli).await {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

async fn run(cli: &Cli) -> Result<()> {
    match &cli.command {
        None => {
            println!("teams-cli {}", env!("CARGO_PKG_VERSION"));
            println!("No command given — try `teams-cli --help`.");
            Ok(())
        }
        Some(Command::Auth { action }) => auth(action, cli.verbose).await,
        Some(Command::Conversations { page_size, raw_out }) => {
            list_conversations(*page_size, raw_out.as_deref()).await
        }
        Some(Command::History { conv, page_size, root, message, raw_out }) => {
            history(conv, *page_size, root.as_deref(), message.as_deref(), raw_out.as_deref()).await
        }
        Some(Command::Roster { raw_out }) => roster(raw_out.as_deref()).await,
        Some(Command::Search { query, limit, raw_out }) => {
            search_people(query, *limit, raw_out.as_deref()).await
        }
    }
}

/// `teams-cli history --conv <id> [--root <id> | --message <id>]`.
///
/// Plain: the conversation's messages page (typed). `--root`: the thread — root plus
/// replies — through the `;messageid=` path form. `--message`: a single message.
async fn history(
    conv: &str,
    page_size: u32,
    root: Option<&str>,
    message: Option<&str>,
    raw_out: Option<&std::path::Path>,
) -> Result<()> {
    if root.is_some() && message.is_some() {
        anyhow::bail!("--root and --message are mutually exclusive");
    }

    let cache = TokenCache::load()?.context("not signed in — run `teams-cli auth login`")?;
    let session = cache.authz.as_ref().context("no authz session — run `teams-cli auth authz`")?;
    let client = teams_api::ChatClient::from_session(session)?
        .with_refresher(teams_api::AuthzRefresher::new()?);

    if let Some(root) = root {
        let thread = client.get_thread(conv, root, page_size).await.context("fetching thread")?;
        dump_raw(&serde_json::to_value(&thread)?, raw_out)?;
        print_thread_summary(&thread, root);
        return Ok(());
    }
    if let Some(id) = message {
        let msg = client.get_message(conv, id).await.context("fetching message")?;
        dump_raw(&serde_json::to_value(&msg)?, raw_out)?;
        println!("single message (bare object, no page envelope)");
        print_message_line(&msg);
        return Ok(());
    }

    let page = client.list_messages(conv, page_size).await.context("listing messages")?;
    dump_raw(&serde_json::to_value(&page)?, raw_out)?;
    print_message_summary(&page);
    Ok(())
}

/// `teams-cli roster` — team/channel list via CSA (Phase 4 read path).
async fn roster(raw_out: Option<&std::path::Path>) -> Result<()> {
    let mut cache = TokenCache::load()?.context("not signed in — run `teams-cli auth login`")?;
    let session = cache.authz.clone().context("no authz session — run `teams-cli auth authz`")?;

    // Bearer for the chatsvcagg audience, minted from the family refresh token on first use.
    let bearer = cache
        .fresh_or_redeem(AUDIENCE_CSA, SCOPE_CSA, "common")
        .await
        .context("minting CSA bearer")?;
    cache.save().context("saving token cache")?;

    let client = teams_api::RosterClient::from_session(&session, bearer)?
        .with_refresher(teams_api::AudienceRefresher::new(AUDIENCE_CSA, SCOPE_CSA)?);
    let roster = client.roster().await.context("fetching CSA roster")?;
    dump_raw(&serde_json::to_value(&roster)?, raw_out)?;
    print_roster_summary(&roster);
    Ok(())
}

/// `teams-cli search <query>` — people directory via substrate (Phase 4 read path).
async fn search_people(query: &str, limit: u32, raw_out: Option<&std::path::Path>) -> Result<()> {
    let mut cache = TokenCache::load()?.context("not signed in — run `teams-cli auth login`")?;

    // Bearer for the substrate audience, minted from the family refresh token on first use.
    let bearer = cache
        .fresh_or_redeem(AUDIENCE_SUBSTRATE, SCOPE_SUBSTRATE, "common")
        .await
        .context("minting substrate bearer")?;
    cache.save().context("saving token cache")?;

    let client = teams_api::SubstrateClient::new(bearer)?
        .with_refresher(teams_api::AudienceRefresher::new(AUDIENCE_SUBSTRATE, SCOPE_SUBSTRATE)?);
    let hits = client.search_people(query, limit).await.context("searching people")?;
    dump_raw(&serde_json::to_value(&hits)?, raw_out)?;
    print_search_summary(&hits);
    Ok(())
}

/// Directory hits at a glance. Values print to the user's own terminal only.
fn print_search_summary(hits: &teams_protocol::SearchSuggestions) {
    let people: Vec<&teams_protocol::PersonSuggestion> = hits.people().collect();
    println!(
        "{} suggestion(s) · best-match order: {}",
        people.len(),
        hits.best_match_order().len()
    );
    for hit in people {
        let name = hit.display_name.as_deref().or(hit.text.as_deref()).unwrap_or("<unnamed>");
        let mut segments: Vec<String> = vec![name.to_owned()];
        segments.extend(
            [hit.job_title.as_deref(), hit.department.as_deref(), hit.email()]
                .into_iter()
                .flatten()
                .map(str::to_owned),
        );
        if !hit.property_hits.is_empty() {
            segments.push(format!("matched {}", hit.property_hits.join(", ")));
        }
        println!("  {}", segments.join(" · "));
    }
}

/// Teams, channels and upcoming chats at a glance. Values print to the user's own terminal.
fn print_roster_summary(roster: &teams_protocol::RosterUpdate) {
    println!(
        "{} teams · {} channels (merged) · {} chats · partial={}",
        roster.teams.len(),
        roster.all_channels().len(),
        roster.chats.len(),
        roster.metadata.is_partial_data.unwrap_or(false),
    );
    for team in &roster.teams {
        let counts =
            team.membership_summary.as_ref().map_or(0, |s| s.total_member_count.unwrap_or(0));
        let channels = team.channels.len();
        let general = team.general_channel().is_some();
        println!(
            "  [{}] {} · channels={} (general={general}) · members={counts} · id={}",
            team.member_role_string.as_deref().unwrap_or("-"),
            team.display_name.as_deref().unwrap_or("<untitled>"),
            channels,
            team.id.as_deref().unwrap_or("-"),
        );
    }
    let meetings =
        roster.chats.iter().filter(|chat| chat.chat_type.as_deref() == Some("meeting")).count();
    println!(
        "  chats: {meetings} meeting(s), {} other · next: {}",
        roster.chats.len() - meetings,
        roster
            .chats
            .iter()
            .filter(|chat| chat.chat_type.as_deref() == Some("meeting"))
            .filter_map(|chat| chat.meeting_information.as_ref())
            .filter(|info| !info.is_cancelled.unwrap_or(true))
            .map(|info| {
                format!(
                    "{} → {}",
                    info.start_time.as_deref().unwrap_or("?"),
                    info.end_time.as_deref().unwrap_or("?")
                )
            })
            .next()
            .unwrap_or_else(|| "-".into()),
    );
}

/// Write a response to `--raw-out` (gitignored `docs/traces/` only — never commit).
fn dump_raw(value: &serde_json::Value, raw_out: Option<&std::path::Path>) -> Result<()> {
    if let Some(path) = raw_out {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(value)?)?;
        eprintln!("response written to {} (do not commit)", path.display());
    }
    Ok(())
}

/// Human-readable messages summary. Values print to the user's own terminal only.
fn print_message_summary(page: &teams_protocol::MessagePage) {
    let backward = if page.metadata.sync_state.is_some() {
        "older history available"
    } else {
        "start of history"
    };
    println!(
        "{} messages ({backward}, tenant present: {})",
        page.messages.len(),
        page.tenant_id.is_some()
    );
    for msg in &page.messages {
        print_message_line(msg);
    }
}

/// Thread summary: root + replies, with counts.
fn print_thread_summary(thread: &teams_protocol::MessagePage, root_id: &str) {
    let is_root = |m: &teams_protocol::Message| m.id == root_id;
    let roots = thread.messages.iter().filter(|m| is_root(m)).count();
    let replies = thread.messages.iter().filter(|m| !is_root(m)).count();
    let paging = if thread.metadata.sync_state.is_some() {
        "more replies available"
    } else {
        "complete thread"
    };
    println!(
        "thread {root_id}: {} messages ({roots} root + {replies} replies, {paging})",
        thread.messages.len()
    );
    for msg in &thread.messages {
        if is_root(msg) {
            println!("  ── root ──");
        }
        print_message_line(msg);
    }
}

/// One message, two lines. Values print to the user's own terminal only.
fn print_message_line(msg: &teams_protocol::Message) {
    let kind = msg.message_type.as_deref().unwrap_or("-");
    let when = msg.original_arrival_time.as_deref().unwrap_or("-");
    let who = msg.im_display_name.as_deref().unwrap_or("<unknown>");
    let mut marks = String::new();
    if msg.is_system() {
        marks.push_str(" [system]");
    }
    if msg.is_edited() {
        marks.push_str(" [edited]");
    }
    if msg.is_reply() {
        marks.push_str(" [reply]");
    }
    let reactions: Vec<String> = msg.emotions().into_iter().map(|e| e.key).collect();
    let reactions = if reactions.is_empty() {
        String::new()
    } else {
        format!(" · reactions={}", reactions.join(","))
    };
    let mentions = msg.mentions().len();
    println!("  seq={:<6} {kind} · {when} · {who}{marks}{reactions}", msg.sequence_id.unwrap_or(0));
    println!(
        "           id={} · root={} · mentions={} · content {} chars",
        msg.id,
        msg.root_message_id.as_deref().unwrap_or("-"),
        mentions,
        msg.content.as_deref().map_or(0, str::len)
    );
}

/// `teams-cli conversations` — live chat-service call (Phase 2A exit criterion).
async fn list_conversations(page_size: u32, raw_out: Option<&std::path::Path>) -> Result<()> {
    let cache = TokenCache::load()?.context("not signed in — run `teams-cli auth login`")?;
    let session = cache.authz.as_ref().context("no authz session — run `teams-cli auth authz`")?;

    let client = teams_api::ChatClient::from_session(session)?
        .with_refresher(teams_api::AuthzRefresher::new()?);
    let response = client.list_conversations(page_size).await.context("listing conversations")?;

    if let Some(path) = raw_out {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(&response)?)?;
        eprintln!("response written to {} (do not commit)", path.display());
    }

    print_conversation_summary(&response);
    Ok(())
}

/// Human-readable page summary. Values print to the user's own terminal only.
fn print_conversation_summary(list: &teams_protocol::ConversationList) {
    let paging = match list.metadata.forward_link.as_deref() {
        Some("") | None => "last page",
        Some(_) => "more pages available",
    };
    println!(
        "{} conversations (totalCount={:?}, {paging})",
        list.conversations.len(),
        list.metadata.total_count
    );
    for conv in &list.conversations {
        let product = conv.product_thread_type().unwrap_or("-");
        let topic = conv.topic().unwrap_or("<untitled>");
        let last_activity = conv
            .last_message
            .as_ref()
            .and_then(|m| m.original_arrival_time.as_deref())
            .unwrap_or("-");
        println!("  [{}] {} · id={}", conv.conversation_type, product, conv.id);
        println!("      topic: {topic} · last activity: {last_activity}");
    }
}

async fn auth(action: &AuthAction, verbose: u8) -> Result<()> {
    match action {
        AuthAction::Login { tenant, scope } => auth_login(tenant, scope.as_deref()).await,
        AuthAction::Status { all_services } => auth_status(*all_services || verbose > 0),
        AuthAction::Authz => auth_authz().await,
        AuthAction::Mint { scope, audience, tenant } => {
            auth_mint(
                scope.as_deref().unwrap_or(SCOPE_CSA),
                audience.as_deref().unwrap_or(AUDIENCE_CSA),
                tenant,
            )
            .await
        }
        AuthAction::Whoami => auth_whoami(),
        AuthAction::Logout => auth_logout(),
    }
}

/// `teams-cli auth login` — device-code flow, then the authz exchange (the spike goal).
async fn auth_login(tenant: &str, scope: Option<&str>) -> Result<()> {
    let scope = scope.unwrap_or(SCOPE_SPACES);
    let client = DeviceCodeClient::new(tenant, FOCI_CLIENT_ID);

    let request = client.request_code(scope).await.context("requesting a device code")?;
    eprintln!("Open: {}", request.login_hint());
    eprintln!("Code: {} (expires in {}s)", request.user_code, request.expires_in);

    let tokens = client.poll_for_token(&request).await.context("device-code sign-in")?;

    let claims = decode_claims(&tokens.access_token).context("decoding access token claims")?;
    let identity = identity_from_claims(&claims).context("reading identity from claims")?;

    let store = teams_auth::default_store().context("selecting credential store")?;
    let mut cache = TokenCache::load_store(&*store)?.unwrap_or_else(TokenCache::new);
    cache.identity = Some(identity.clone());
    cache.put_token(AUDIENCE_SPACES, tokens.clone());

    // Authz is the actual Phase 2A goal: skypetoken + region map.
    match AuthzClient::new().exchange(&tokens.access_token).await {
        Ok(session) => {
            print_authz(&session);
            cache.authz = Some(session);
        }
        Err(err) => eprintln!("warning: authz exchange failed (tokens are cached): {err:#}"),
    }
    cache.save_store(&*store).context("saving token cache")?;

    println!("signed in{}", identity_suffix(&identity));
    println!("credentials: {}", store.describe());
    Ok(())
}

/// `teams-cli auth status` — cache state, expiry times, authz summary.
fn auth_status(all_services: bool) -> Result<()> {
    let store = teams_auth::default_store().context("selecting credential store")?;
    let Some(cache) = TokenCache::load_store(&*store)? else {
        println!("not signed in (no credentials in {})", store.describe());
        return Ok(());
    };

    println!("store: {}", store.describe());
    match &cache.identity {
        Some(identity) => println!("signed in{}", identity_suffix(identity)),
        None => println!("no identity recorded"),
    }
    if let Some(updated) = cache.updated_at {
        println!("cache updated: {}", updated.to_rfc3339());
    }

    for (audience, token) in &cache.tokens {
        let refresh =
            if token.refresh_token.is_some() { "refresh token: yes" } else { "refresh token: NO" };
        println!("  {audience}: expires {} ({refresh})", token.expires_at().to_rfc3339());
    }

    match &cache.authz {
        Some(session) => print_authz_detail(session, all_services),
        None => println!("no authz session yet — run `teams-cli auth authz`"),
    }
    Ok(())
}

/// `teams-cli auth authz` — refresh if needed, exchange, persist.
async fn auth_authz() -> Result<()> {
    let mut cache = TokenCache::load()
        .context("reading token cache")?
        .context("not signed in — run `teams-cli auth login` first")?;

    let access = fresh_spaces_token(&mut cache).await?;
    // Persist a refreshed token even if the exchange below fails.
    cache.save().context("saving token cache")?;

    let session = AuthzClient::new().exchange(&access).await.context("authz exchange")?;
    print_authz(&session);
    cache.authz = Some(session);
    cache.save().context("saving token cache")?;
    Ok(())
}

/// `teams-cli auth mint` — redeem the cached family refresh token for another audience
/// (per-audience bearer: CSA roster, substrate, …). Prints claims, never the token.
async fn auth_mint(scope: &str, audience: &str, tenant: &str) -> Result<()> {
    let mut cache = TokenCache::load()?.context("not signed in — run `teams-cli auth login`")?;
    let used_cache = cache.fresh_token(audience).is_some();

    let access = cache.fresh_or_redeem(audience, scope, tenant).await.context("minting bearer")?;
    cache.save().context("saving token cache")?;

    let claims = decode_claims(&access).context("decoding minted access token claims")?;
    let field =
        |name: &str| claims.get(name).and_then(serde_json::Value::as_str).unwrap_or("-").to_owned();
    let source = if used_cache { "cached" } else { "redeemed via refresh token" };
    println!("{audience}: {source} · aud={} · scp={} · exp={}", field("aud"), field("scp"), {
        claims.get("exp").and_then(serde_json::Value::as_i64).unwrap_or(0)
    });
    Ok(())
}

/// `teams-cli auth whoami` — print (unverified) claims of the cached spaces token.
fn auth_whoami() -> Result<()> {
    let cache = TokenCache::load()?.context("not signed in — run `teams-cli auth login` first")?;
    let token = cache.token(AUDIENCE_SPACES)?;
    let claims = decode_claims(&token.access_token).context("decoding access token claims")?;
    println!("{}", serde_json::to_string_pretty(&claims)?);
    Ok(())
}

/// `teams-cli auth logout` — delete the credentials in the active store.
fn auth_logout() -> Result<()> {
    let store = teams_auth::default_store().context("selecting credential store")?;
    if TokenCache::clear_store(&*store)? {
        println!("credentials removed from {}", store.describe());
    } else {
        println!("no credentials to remove in {}", store.describe());
    }
    Ok(())
}

/// A non-expired spaces access token, refreshing via the FRT when necessary.
async fn fresh_spaces_token(cache: &mut TokenCache) -> Result<String> {
    if let Some(token) = cache.fresh_token(AUDIENCE_SPACES) {
        return Ok(token.access_token.clone());
    }

    let refresh = cache
        .token(AUDIENCE_SPACES)?
        .refresh_token
        .clone()
        .context("token expired and no refresh token — run `teams-cli auth login`")?;

    tracing::info!("spaces token expired; refreshing via FRT");
    let client = DeviceCodeClient::new("common", FOCI_CLIENT_ID);
    let token = client.refresh(&refresh, SCOPE_SPACES).await.context("refreshing spaces token")?;
    let access = token.access_token.clone();
    cache.put_token(AUDIENCE_SPACES, token);
    Ok(access)
}

fn identity_suffix(identity: &teams_auth::CachedIdentity) -> String {
    match (&identity.name, &identity.upn) {
        (Some(name), Some(upn)) => format!(" as {name} ({upn}), tenant {}", identity.tid),
        (Some(name), None) => format!(" as {name}, tenant {}", identity.tid),
        (None, Some(upn)) => format!(" as {upn}, tenant {}", identity.tid),
        (None, None) => format!(", tenant {}", identity.tid),
    }
}

/// One-line authz summary. Region/hostnames are service metadata, not secrets.
fn print_authz(session: &AuthzSession) {
    let region = session.region.as_deref().unwrap_or("<none>");
    let expires =
        session.expires_in.map_or_else(|| "unknown lifetime".to_owned(), |s| format!("{s}s"));
    println!(
        "authz: region={region}, skype token lifetime {expires}, {} services",
        session.region_gtms.len()
    );
    if let Some(chat) = session.host("chatService") {
        println!("  chatService = {chat}");
    }
}

fn print_authz_detail(session: &AuthzSession, all_services: bool) {
    print_authz(session);
    if all_services {
        for name in session.service_names() {
            match session.host(name) {
                Some(host) => println!("  {name} = {host}"),
                None => println!("  {name} = <non-string entry>"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn broken_pipe_panics_are_recognised() {
        // The exact shape std's `println!` produces on EPIPE.
        let owned = String::from("failed printing to stdout: Broken pipe (os error 32)");
        assert!(is_broken_pipe(&owned));
        assert!(is_broken_pipe(&"failed printing to stdout: Broken pipe (os error 32)"));

        // Everything else must fall through to the default panic reporting.
        assert!(!is_broken_pipe(&String::from("index out of bounds: the len is 3")));
        assert!(!is_broken_pipe(&"assertion failed: left == right"));
    }

    #[test]
    fn identity_suffix_handles_missing_fields() {
        let identity = teams_auth::CachedIdentity { tid: "tid-1".into(), upn: None, name: None };
        assert_eq!(identity_suffix(&identity), ", tenant tid-1");

        let identity = teams_auth::CachedIdentity {
            tid: "tid-1".into(),
            upn: Some("u@example.com".into()),
            name: Some("User".into()),
        };
        assert_eq!(identity_suffix(&identity), " as User (u@example.com), tenant tid-1");
    }
}
