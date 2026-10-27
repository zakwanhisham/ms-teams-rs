//! People-search wire types — `POST /search/api/v1/suggestions?scenario=powerbar`.
//!
//! Field names are documented verbatim from a live response (2026-10-27, sanitized
//! fixture: `tests/fixtures/search.json`, query matched three people). Same tolerance
//! rules as [`crate::conversations`]: everything optional/defaulted, unknown fields kept
//! in the flattened `extra` maps, no I/O in this crate.
//!
//! Two shapes this module exists to absorb:
//! - the response is **grouped** (`Groups[].Suggestions[]`) rather than a flat list, and
//!   the group carries its entity type (`"Type": "People"`); other entity types
//!   (files, messages, …) may arrive in the same envelope — [`SearchSuggestions::people`]
//!   only flattens the groups the caller asked for by filtering on that type;
//! - `LayoutHints.BestMatchSuggestions` **references** suggestions by `ReferenceId`
//!   instead of inlining them, so ordering lives there.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Root of a suggestions response.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SearchSuggestions {
    /// Result groups; one `People` group is returned for an `EntityType: "People"` request.
    #[serde(rename = "Groups", default)]
    pub groups: Vec<SuggestionsGroup>,
    /// Server-side layout ordering (which suggestions to show as "best match").
    #[serde(rename = "LayoutHints", default)]
    pub layout_hints: Option<LayoutHints>,
    /// Correlation id of the serving cluster (wire key `Instrumentation.TraceId`).
    #[serde(rename = "Instrumentation", default)]
    pub instrumentation: Option<SearchInstrumentation>,
    /// Any root-level fields we don't know yet.
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

/// One group of suggestions (`Groups[]`), typed by [`SuggestionsGroup::group_type`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SuggestionsGroup {
    /// The suggestions of this group.
    #[serde(rename = "Suggestions", default)]
    pub suggestions: Vec<PersonSuggestion>,
    /// Entity type of the group — `"People"` for the person search.
    #[serde(rename = "Type", default)]
    pub group_type: Option<String>,
    /// Unknown fields.
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

/// One person suggestion (a directory hit).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PersonSuggestion {
    /// Stable suggestion id (`<guid>@<tenant-guid>` observed — one per directory object).
    #[serde(rename = "Id", default)]
    pub id: Option<String>,
    /// Member resource id in MRI form (`8:orgid:<guid>` observed).
    #[serde(rename = "MRI", default)]
    pub mri: Option<String>,
    /// Display name as shown in the pickers.
    #[serde(rename = "DisplayName", default)]
    pub display_name: Option<String>,
    /// Same text as [`PersonSuggestion::display_name`] — the string the picker renders.
    #[serde(rename = "Text", default)]
    pub text: Option<String>,
    /// What the suggestion matched on, e.g. the email that was completed.
    #[serde(rename = "QueryText", default)]
    pub query_text: Option<String>,
    /// Email addresses; populated when `EmailAddresses` was requested in `Fields`.
    #[serde(rename = "EmailAddresses", default)]
    pub email_addresses: Vec<String>,
    /// Job title, when the directory provides one.
    #[serde(rename = "JobTitle", default)]
    pub job_title: Option<String>,
    /// Department, when the directory provides one.
    #[serde(rename = "Department", default)]
    pub department: Option<String>,
    /// Which directory properties matched the query (`DisplayName`, `GivenName`,
    /// `proxy_email` observed) — request-side `Fields` control what can appear here.
    #[serde(rename = "PropertyHits", default)]
    pub property_hits: Vec<String>,
    /// Opaque id used by [`LayoutHints`] to order this suggestion.
    #[serde(rename = "ReferenceId", default)]
    pub reference_id: Option<String>,
    /// Unknown fields.
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

/// `LayoutHints`: presentation order, referencing suggestions by id.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LayoutHints {
    /// Ordered ids of the suggestions to surface first.
    #[serde(rename = "BestMatchSuggestions", default)]
    pub best_match_suggestions: Vec<BestMatchHint>,
    /// Unknown fields.
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

/// One entry of `LayoutHints.BestMatchSuggestions`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct BestMatchHint {
    /// `ReferenceId` of the suggestion this hint points at.
    #[serde(rename = "ReferenceId", default)]
    pub reference_id: Option<String>,
    /// Group the referenced suggestion belongs to (`"People"` observed).
    #[serde(default)]
    pub group: Option<String>,
    /// Unknown fields.
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

/// `Instrumentation`: server-side correlation for the suggestions call.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SearchInstrumentation {
    /// Trace id to quote when reporting a bad suggestions response.
    #[serde(rename = "TraceId", default)]
    pub trace_id: Option<String>,
    /// Unknown fields.
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

impl SearchSuggestions {
    /// Every suggestion of the group whose `Type` equals `group_type`, in wire order.
    pub fn group_of<'a>(&'a self, group_type: &str) -> impl Iterator<Item = &'a PersonSuggestion> {
        self.groups
            .iter()
            .filter(move |group| group.group_type.as_deref() == Some(group_type))
            .flat_map(|group| group.suggestions.iter())
    }

    /// All person suggestions (`Type: "People"` groups), in wire order.
    pub fn people(&self) -> impl Iterator<Item = &PersonSuggestion> {
        self.group_of("People")
    }

    /// The suggested ordering of `LayoutHints.BestMatchSuggestions`, as
    /// [`PersonSuggestion::reference_id`] values (unresolvable ids are skipped).
    #[must_use]
    pub fn best_match_order(&self) -> Vec<&str> {
        self.layout_hints
            .as_ref()
            .map(|hints| {
                hints
                    .best_match_suggestions
                    .iter()
                    .filter_map(|hint| hint.reference_id.as_deref())
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl PersonSuggestion {
    /// Primary email address (the first of [`PersonSuggestion::email_addresses`]).
    #[must_use]
    pub fn email(&self) -> Option<&str> {
        self.email_addresses.first().map(String::as_str)
    }

    /// True when `property` is one of the directory properties this hit matched on.
    #[must_use]
    pub fn matched(&self, property: &str) -> bool {
        self.property_hits.iter().any(|hit| hit == property)
    }
}
