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
    AUDIENCE_SPACES, AuthzClient, AuthzSession, DeviceCodeClient, FOCI_CLIENT_ID, SCOPE_SPACES,
    TokenCache, decode_claims, identity_from_claims,
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
    /// Decode (unverified) claims of the cached spaces token
    Whoami,
    /// Delete the local token cache
    Logout,
}

#[tokio::main]
async fn main() {
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
    let client = teams_api::ChatClient::from_session(session)?;

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

    let client = teams_api::ChatClient::from_session(session)?;
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

    let mut cache = TokenCache::load()?.unwrap_or_else(TokenCache::new);
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
    cache.save().context("saving token cache")?;

    println!("signed in{}", identity_suffix(&identity));
    println!("token cache: {}", TokenCache::path().display());
    Ok(())
}

/// `teams-cli auth status` — cache state, expiry times, authz summary.
fn auth_status(all_services: bool) -> Result<()> {
    let Some(cache) = TokenCache::load()? else {
        println!("not signed in (no cache at {})", TokenCache::path().display());
        return Ok(());
    };

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

/// `teams-cli auth whoami` — print (unverified) claims of the cached spaces token.
fn auth_whoami() -> Result<()> {
    let cache = TokenCache::load()?.context("not signed in — run `teams-cli auth login` first")?;
    let token = cache.token(AUDIENCE_SPACES)?;
    let claims = decode_claims(&token.access_token).context("decoding access token claims")?;
    println!("{}", serde_json::to_string_pretty(&claims)?);
    Ok(())
}

/// `teams-cli auth logout` — delete the cache file.
fn auth_logout() -> Result<()> {
    if TokenCache::clear()? {
        println!("token cache removed");
    } else {
        println!("no token cache to remove");
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
