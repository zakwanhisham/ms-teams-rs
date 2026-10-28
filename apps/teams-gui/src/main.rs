//! `teams-gui` — native Teams desktop client (Phase 6). Toolkit: **iced 0.14**
//! (decision locked 2026-10-29, `TODO.md` §0): Elm-style MVU, retained mode. All state
//! in [`State`], all mutation in [`update`]; network hops are `Task`s or subscription
//! ticks built on the live-verified `teams-api` read/write paths — the GUI never does
//! I/O inline.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use iced::widget::Column as IcedColumn;
use iced::widget::{button, column, row, scrollable, text, text_input};
use iced::{Element, Fill, Subscription, Task};
use teams_api::ChatClient;
use teams_protocol::Message;

/// One row of the conversations sidebar.
#[derive(Clone, Debug)]
struct ConvRow {
    id: String,
    label: String,
    preview: Option<String>,
}

/// One row of the message list (plain-text projection for this slice).
#[derive(Clone, Debug)]
struct MsgRow {
    #[expect(dead_code, reason = "message addressing (reply/edit) is the next slice")]
    id: String,
    sender: String,
    body: String,
}

/// MVU state.
#[derive(Debug, Default)]
struct State {
    selected: Option<String>,
    convs: Vec<ConvRow>,
    msgs: Vec<MsgRow>,
    draft: String,
    error: Option<String>,
}

/// Messages driving the MVU loop.
#[derive(Debug, Clone)]
enum AppMessage {
    /// Conversations refresh landed (rows or error) — also the boot tick.
    Conversations(Result<Vec<ConvRow>, String>),
    /// A conversation was picked from the sidebar.
    Select(String),
    /// A message page landed (newest page of the selected conversation).
    Messages(Vec<MsgRow>),
    /// Poll tick; refreshes the selected conversation.
    PollTick,
    /// Composer keystrokes.
    Draft(String),
    /// Send the draft.
    Send,
    /// Send landed with the server message id (or the failure reason).
    Sent(Result<String, String>),
}

/// `$XDG_STATE_HOME/teams-rs` store base directory (same rules as the CLI).
fn state_dir() -> PathBuf {
    use std::path::Path;
    let base = match std::env::var_os("XDG_STATE_HOME") {
        Some(dir) if !dir.is_empty() && Path::new(&dir).is_absolute() => PathBuf::from(dir),
        _ => std::env::var_os("HOME")
            .map(|home| PathBuf::from(home.to_string_lossy().into_owned()).join(".local/state"))
            .expect("neither XDG_STATE_HOME nor HOME is set"),
    };
    base.join("teams-rs")
}

/// Client from the shared token cache; `Err` when signed out.
fn build_client() -> Result<Arc<ChatClient>, String> {
    let cache = teams_auth::TokenCache::load().map_err(|e| e.to_string())?;
    let session =
        cache.and_then(|c| c.authz).ok_or("not signed in — run `teams-cli auth login`")?;
    let _ = state_dir();
    let client = ChatClient::from_session(&session)
        .map_err(|e| e.to_string())?
        .with_refresher(teams_api::AuthzRefresher::new().map_err(|e| e.to_string())?);
    Ok(Arc::new(client))
}

/// First line of a message body as plain text (mentions collapsed for this slice).
fn body_preview(msg: &Message) -> String {
    let rich = teams_protocol::richtext::parse(msg.content.as_deref().unwrap_or(""));
    rich.to_plain_text().lines().next().unwrap_or("").to_owned()
}

fn msg_row(msg: &Message) -> MsgRow {
    MsgRow {
        id: msg.id.clone(),
        sender: msg
            .im_display_name
            .clone()
            .or_else(|| msg.from.clone())
            .unwrap_or_else(|| "?".into()),
        body: body_preview(msg),
    }
}

/// Wire page (newest-first) → rows oldest-first for rendering.
fn rows_from_page(page: &teams_protocol::MessagePage) -> Vec<MsgRow> {
    let mut rows = Vec::with_capacity(page.messages.len());
    for msg in &page.messages {
        rows.push(msg_row(msg));
    }
    rows.reverse();
    rows
}

/// Human label for a conversation: wire topic → space topic → original thread id →
/// raw id, in that preference order (matches the CLI's `topic:` printing).
fn conv_label(c: &teams_protocol::Conversation) -> String {
    let props = c.thread_properties.as_ref();
    let label = props
        .and_then(|p| p.topic.clone())
        .or_else(|| props.and_then(|p| p.space_thread_topic.clone()))
        .unwrap_or_default();
    if label.is_empty() { c.id.clone() } else { label }
}

fn conv_row(c: &teams_protocol::Conversation) -> ConvRow {
    ConvRow {
        id: c.id.clone(),
        label: conv_label(c),
        preview: c.last_message.as_ref().map(body_preview),
    }
}

fn fetch_conversations() -> Task<AppMessage> {
    Task::future(async move {
        match build_client() {
            Ok(client) => AppMessage::Conversations(
                client
                    .list_conversations(20)
                    .await
                    .map_err(|e| e.to_string())
                    .map(|list| list.conversations.iter().map(conv_row).collect()),
            ),
            Err(e) => AppMessage::Conversations(Err(e)),
        }
    })
}

fn fetch_messages(client: Arc<ChatClient>, conv: String) -> Task<AppMessage> {
    Task::future(async move {
        match client.list_messages(&conv, 25).await {
            Ok(page) => AppMessage::Messages(rows_from_page(&page)),
            Err(e) => AppMessage::Sent(Err(format!("list_messages: {e}"))),
        }
    })
}

fn send_message(client: Arc<ChatClient>, conv: String, diver: String) -> Task<AppMessage> {
    Task::future(async move {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_millis()).ok())
            .unwrap_or(0);
        match client.send_message(&conv, &diver, stamp).await {
            Ok(sent) => AppMessage::Sent(Ok(sent.message_id.as_deref().unwrap_or("?").to_owned())),
            Err(e) => AppMessage::Sent(Err(format!("send: {e}"))),
        }
    })
}

fn update(state: &mut State, message: AppMessage) -> Task<AppMessage> {
    match message {
        AppMessage::Conversations(Ok(rows)) => {
            state.convs = rows;
            Task::none()
        }
        AppMessage::Conversations(Err(e)) | AppMessage::Sent(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        AppMessage::Select(id) => {
            state.selected = Some(id.clone());
            match build_client() {
                Ok(client) => fetch_messages(client, id),
                Err(e) => Task::done(AppMessage::Sent(Err(e))),
            }
        }
        AppMessage::Messages(rows) => {
            state.msgs = rows;
            Task::none()
        }
        AppMessage::PollTick => {
            let Some(conv) = state.selected.clone() else { return Task::none() };
            match build_client() {
                Ok(client) => fetch_messages(client, conv),
                Err(_) => Task::none(), // signed out mid-session; the sidebar says why
            }
        }
        AppMessage::Draft(s) => {
            state.draft = s;
            Task::none()
        }
        AppMessage::Send => {
            let Some(conv) = state.selected.clone() else { return Task::none() };
            let body = state.draft.clone();
            if body.trim().is_empty() {
                return Task::none();
            }
            state.draft.clear();
            match build_client() {
                Ok(client) => {
                    let escaped = html_escape(&body);
                    send_message(client.clone(), conv.clone(), format!("<div>{escaped}</div>"))
                        .chain(fetch_messages(client, conv))
                }
                Err(e) => Task::done(AppMessage::Sent(Err(e))),
            }
        }
        AppMessage::Sent(Ok(_id)) => {
            state.error = None;
            Task::none()
        }
    }
}

fn sidebar(state: &State) -> Element<'_, AppMessage> {
    let header = text("conversations").size(14);
    let list = state.convs.iter().fold(column![header].spacing(4), |acc, c| {
        let mut row = acc.push(
            button(text(c.label.clone()).size(13))
                .on_press(AppMessage::Select(c.id.clone()))
                .width(Fill),
        );
        if c.preview.as_ref().is_some_and(|p| !p.is_empty()) {
            row = row.push(text(c.preview.clone().expect("checked")).size(11));
        }
        row
    });
    scrollable(list).width(280).into()
}

fn message_list(state: &State) -> Element<'_, AppMessage> {
    let list =
        state.msgs.iter().fold(column![].spacing(8), |acc: IcedColumn<'_, AppMessage>, m| {
            acc.push(
                column![text(m.sender.clone()).size(12), text(m.body.clone()).size(14),].spacing(2),
            )
        });
    scrollable(list).width(Fill).height(Fill).into()
}

fn composer(state: &State) -> Element<'_, AppMessage> {
    row![
        text_input("write a message", &state.draft)
            .on_input(AppMessage::Draft)
            .on_submit(AppMessage::Send)
            .width(Fill),
        button("send").on_press_maybe((!state.draft.trim().is_empty()).then_some(AppMessage::Send)),
    ]
    .spacing(8)
    .padding(8)
    .into()
}

fn view(state: &State) -> Element<'_, AppMessage> {
    let status = match (&state.selected, &state.error) {
        (_, Some(e)) => text(format!("⚠ {e}")).size(12),
        (Some(c), None) => text(format!("selected: {c}")).size(12),
        (None, None) => text("pick a conversation").size(12),
    };
    column![
        row![sidebar(state), message_list(state)].height(Fill).spacing(8),
        status,
        composer(state),
    ]
    .width(Fill)
    .height(Fill)
    .spacing(4)
    .padding(6)
    .into()
}

/// 5 s poll of the selected conversation while one is open (stopgap channel; the
/// streaming Trouter transport lands when the owner capture settles v4 auth).
fn subscription(state: &State) -> Subscription<AppMessage> {
    if state.selected.is_none() {
        return Subscription::none();
    }
    iced::time::every(Duration::from_secs(5)).map(|_| AppMessage::PollTick)
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

pub fn main() -> iced::Result {
    iced::application(boot, update, view).title("teams-gui").subscription(subscription).run()
}

/// Boot: empty state + one conversations refresh (the `Conversations` arm of
/// [`update`] fills the sidebar from it).
fn boot() -> (State, Task<AppMessage>) {
    (State::default(), fetch_conversations())
}
