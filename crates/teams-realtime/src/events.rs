//! Typed event decoding: a wire message (as observed in the committed traces and on
//! read paths) becomes a [`TeamEvent`].
//!
//! The classification is deliberately conservative — a wrong guess is worse than a
//! prominent fallback, so anything not pinned by a live observation falls into
//! [`TeamEvent::Other`] carrying the raw JSON. Shapes classified:
//!
//! - `Control/Typing` → [`TeamEvent::Typing`] (verified live as a send; the receive-side
//!   shape is the same message model, trace frame still pending Trouter auth).
//! - `properties.skypeeditedid` present → [`TeamEvent::MessageEdited`] (the edit marker
//!   equals the edit's own `clientmessageid` — see `docs/endpoints.md`).
//! - `properties.deletetime` / `adminDelete` present → [`TeamEvent::MessageDeleted`]
//!   (delete bookkeeping lives in the property bag).
//! - `properties.emotions` / `deltaEmotions` present → [`TeamEvent::ReactionChanged`]
//!   (`[{key, users:[{mri,time,value}]}]` — verified on read paths).
//! - `messagetype: "Event/Call"` → [`TeamEvent::CallActivity`] (call-log XML in
//!   `content`, verified live 2026-10-28). The inbound-call signal proper rides the
//!   Trouter path, so [`TeamEvent::IncomingCall`] is a separate variant.
//! - `messagetype: "ThreadActivity/*"` → [`TeamEvent::ConversationUpdated`].
//! - `Text` / `RichText/Html` → [`TeamEvent::MessageCreated`] (the normal bubble).
//!
//! [`TeamEvent::Presence`] and [`TeamEvent::EndpointExpired`] have no observed wire
//! shape yet — they can only arrive once the Trouter transport authenticates (owner
//! capture pending). Everything else is [`TeamEvent::Other`].

use serde_json::Value;

use teams_protocol::Message;

/// One decoded realtime event.
#[derive(Debug)]
pub enum TeamEvent {
    /// A normal chat bubble (`Text` / `RichText/Html`, also inline images riding
    /// `RichText/Html` — distinguish with `message.has_inline_media()`).
    MessageCreated {
        /// Conversation the message landed in (`conversationid` or the
        /// `conversationLink` tail when the id is absent).
        conversation_id: String,
        /// The full decoded message (lenient typed model).
        message: Message,
    },
    /// An edit of an existing message. `properties` is replaced on edit, not merged —
    /// the decoded [`Message`] already carries the *new* property set.
    MessageEdited {
        conversation_id: String,
        message: Message,
        /// The edit marker (`properties.skypeeditedid`).
        edit_client_message_id: String,
    },
    /// A delete; the bookkeeping fields ride the property bag
    /// (`adminDelete`, `deletetime`, `hardDelete*`).
    MessageDeleted { conversation_id: String, message: Message },
    /// A reaction was added or removed — the payload is the full emotions list
    /// (`{key, users}` entries), so the consumer diff against the previous state.
    ReactionChanged {
        conversation_id: String,
        message: Message,
        /// Raw `emotions`/`deltaEmotions` property (an array of `{key, users}`).
        emotions: Value,
    },
    /// Someone is typing.
    Typing { conversation_id: String, message: Message },
    /// A call log event (`Event/Call`, XML body; see `docs/endpoints.md`).
    CallActivity { conversation_id: String, message: Message },
    /// Thread/membership/topic change (`ThreadActivity/*`).
    ConversationUpdated { conversation_id: String, message: Message },
    /// Receiving-side presence (no observed wire shape yet).
    Presence,
    /// The transport's endpoint was expired and must re-register (no observed shape
    /// yet; the Trouter reconnect logic names the condition).
    EndpointExpired,
    /// Inbound call signal over Trouter (no observed wire shape yet — see
    /// `docs/calls.md`).
    IncomingCall,
    /// Anything else — kept prominent instead of guessed. Carries the raw JSON.
    Other(Value),
}

impl TeamEvent {
    /// Decode one wire message (`type: "Message"` or any chat-service message shape).
    #[must_use]
    pub fn from_message_json(value: &Value) -> Self {
        let Ok(message) = serde_json::from_value::<Message>(value.clone()) else {
            return Self::Other(value.clone());
        };
        let conversation_id = message
            .conversation_id
            .clone()
            .or_else(|| {
                // Fallback: the conversation link's tail requires a `:` to be a real id
                // (links only exist server-side; a `/`-split tail is a path fragment).
                message.conversation_link.as_deref().and_then(|link| {
                    link.rsplit('/')
                        .next()
                        .and_then(|tail| (tail.contains(':')).then(|| tail.to_owned()))
                })
            })
            .unwrap_or_default();
        let props = &message.properties;
        let edit_marker: Option<String> = props
            .extra
            .get("skypeeditedid")
            .and_then(Value::as_str)
            .or(message.skype_edited_id.as_deref())
            .map(str::to_owned);
        let is_deleted =
            props.extra.contains_key("deletetime") || props.extra.contains_key("adminDelete");
        let reaction = props.emotions.clone().or_else(|| props.delta_emotions.clone());
        let message_type = message.message_type.clone();

        if message_type.as_deref() == Some("Control/Typing") {
            return Self::Typing { conversation_id, message };
        }
        if is_deleted {
            return Self::MessageDeleted { conversation_id, message };
        }
        if let Some(edit_marker) = edit_marker {
            return Self::MessageEdited {
                conversation_id,
                message,
                edit_client_message_id: edit_marker,
            };
        }
        if let Some(emotions) = reaction {
            return Self::ReactionChanged { conversation_id, message, emotions };
        }
        match message_type.as_deref() {
            Some("Event/Call") => Self::CallActivity { conversation_id, message },
            Some(kind) if kind.starts_with("ThreadActivity/") => {
                Self::ConversationUpdated { conversation_id, message }
            }
            Some("Text" | "RichText/Html") => Self::MessageCreated { conversation_id, message },
            _ => Self::Other(value.clone()),
        }
    }

    /// Conversation id of any event carrying a message; `None` otherwise.
    #[must_use]
    pub fn conversation_id(&self) -> Option<&str> {
        match self {
            Self::MessageCreated { conversation_id, .. }
            | Self::MessageEdited { conversation_id, .. }
            | Self::MessageDeleted { conversation_id, .. }
            | Self::ReactionChanged { conversation_id, .. }
            | Self::Typing { conversation_id, .. }
            | Self::CallActivity { conversation_id, .. }
            | Self::ConversationUpdated { conversation_id, .. } => Some(conversation_id),
            _ => None,
        }
    }
}
