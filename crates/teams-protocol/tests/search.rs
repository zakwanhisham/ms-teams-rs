//! Deserialization tests for the substrate suggestions fixture
//! (`tests/fixtures/search.json`).
//!
//! The fixture is a sanitized slice of a live
//! `POST /search/api/v1/suggestions?scenario=powerbar` response (2026-10-27, query
//! matched three directory people, two kept by the array limit). Names, emails, ids and
//! trace ids are `<redacted>`; the protocol-surface enums (`Type`, `PropertyHits`) survive
//! — see `tools/sanitize_fixture.py`.

use teams_protocol::{PersonSuggestion, SearchSuggestions};

const FIXTURE: &str = include_str!("fixtures/search.json");

fn suggestions() -> SearchSuggestions {
    serde_json::from_str(FIXTURE).expect("search fixture deserializes")
}

#[test]
fn fixture_has_the_expected_shape() {
    let page = suggestions();
    assert_eq!(page.groups.len(), 1, "one result group kept");
    assert_eq!(page.groups[0].group_type.as_deref(), Some("People"));
    assert_eq!(page.people().count(), 2, "two suggestions kept by the array limit");
    assert_eq!(page.layout_hints.as_ref().expect("layout hints").best_match_suggestions.len(), 2);
    assert!(page.instrumentation.is_some(), "trace id survives (redacted value)");
    assert!(page.extra.is_empty(), "all three root keys are typed");
}

#[test]
fn suggestions_carry_the_directory_fields_we_asked_for() {
    let page = suggestions();
    let people: Vec<&PersonSuggestion> = page.people().collect();
    let (first, second) = (people[0], people[1]);

    for hit in &people {
        assert_eq!(hit.id.as_deref(), Some("<redacted>"));
        assert_eq!(hit.mri.as_deref(), Some("<redacted>"));
        assert_eq!(hit.display_name.as_deref(), Some("<redacted>"));
        // `Text` mirrors `DisplayName` and `QueryText` carries the matched email.
        assert_eq!(hit.text, hit.display_name, "Text duplicates DisplayName on the wire");
        assert_eq!(hit.query_text.as_deref(), Some("<redacted>"));
        assert_eq!(hit.email().map(str::is_empty), Some(false), "one address each");
        assert_eq!(hit.email_addresses.len(), 1);
        assert_eq!(hit.reference_id.as_deref(), Some("<redacted>"));
    }

    // Directory attributes are optional: the first hit has no department live.
    assert_eq!(first.job_title.as_deref(), Some("<redacted>"));
    assert_eq!(first.department, None);
    assert_eq!(second.department.as_deref(), Some("<redacted>"));

    // `PropertyHits` is enum-like and kept verbatim by the sanitizer.
    assert!(first.matched("DisplayName"));
    assert!(!first.matched("proxy_email"));
    assert!(second.matched("proxy_email"));
}

#[test]
fn people_only_flattens_people_groups() {
    let page = suggestions();
    assert_eq!(page.group_of("People").count(), 2);
    assert_eq!(page.group_of("Files").count(), 0, "an entity type we never asked for");

    // A differently-typed group must not leak into `people()`.
    let mut page = page;
    page.groups.push(teams_protocol::SuggestionsGroup {
        group_type: Some("Files".into()),
        suggestions: vec![PersonSuggestion::default()],
        ..teams_protocol::SuggestionsGroup::default()
    });
    assert_eq!(page.people().count(), 2, "the Files group is skipped");
    assert_eq!(page.group_of("Files").count(), 1);
}

#[test]
fn best_match_order_matches_the_reference_ids() {
    let page = suggestions();
    let order = page.best_match_order();
    assert_eq!(order.len(), 2, "one hint per surfaced suggestion");

    let reference_ids: Vec<&str> =
        page.people().filter_map(|hit| hit.reference_id.as_deref()).collect();
    for id in order {
        assert!(reference_ids.contains(&id), "every hint resolves to a suggestion");
    }
}

/// The service keeps adding fields; a response with an unknown key must still parse and
/// keep it in the catch-all.
#[test]
fn unknown_fields_are_preserved_at_every_level() {
    let raw = serde_json::json!({
        "Groups": [{
            "Type": "People",
            "NewGroupField": 1,
            "Suggestions": [{
                "DisplayName": "n",
                "SomethingNew": {"a": 1},
                "PropertyHits": ["DisplayName"]
            }]
        }],
        "NewRootField": "x"
    });
    let page: SearchSuggestions = serde_json::from_value(raw).expect("unknown fields tolerated");

    assert!(page.extra.contains_key("NewRootField"));
    assert!(page.groups[0].extra.contains_key("NewGroupField"));
    let hit = &page.groups[0].suggestions[0];
    assert!(hit.extra.contains_key("SomethingNew"));
    assert_eq!(hit.property_hits, vec!["DisplayName".to_owned()]);
}

/// Every field is optional: `{}` is a valid (empty) answer, not a parse error.
#[test]
fn empty_object_is_an_empty_result() {
    let page: SearchSuggestions = serde_json::from_str("{}").expect("empty page");
    assert!(page.people().next().is_none());
    assert_eq!(page.best_match_order(), [] as [&str; 0], "no ordering without layout hints");
    assert_eq!(page.layout_hints, None);
}

#[test]
fn roundtrip_is_lossless() {
    let page = suggestions();
    let again = serde_json::to_string(&page).expect("serialize");
    let back: SearchSuggestions = serde_json::from_str(&again).expect("reparse");
    assert_eq!(page, back);
}
