// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use chrono::NaiveDate;
use herdr_agentsview::wire::{
    AgentInfo, AgentsResponse, Automation, ProjectInfo, ProjectsResponse, Report, ReportSelection,
    TimingQuality, ACTIVITY_SCHEMA_VERSION,
};

// Contract recorded from kenn-io/agentsview revision
// 5ae0f872d60357216d18373883953308c66b194f.
#[test]
fn report_v6_fixture_decodes_exact_contract() {
    // If AgentsView changes the versioned response shape without coordinated client work,
    // the dashboard must fail at the boundary instead of rendering invented defaults.
    let report: Report = serde_json::from_str(include_str!("fixtures/report-v6.json"))
        .expect("recorded schema-v6 fixture must decode");

    assert_eq!(report.schema_version, ACTIVITY_SCHEMA_VERSION);
    assert_eq!(report.report_id.as_deref(), Some("fixture-report-id"));
    assert_eq!(report.sessions_total, report.by_session.len());
    assert_eq!(report.totals.sessions, 3);
    assert_eq!(report.by_session[2].timing_quality, TimingQuality::Untimed);
    assert_eq!(
        report.buckets[0].interactive_at_peak + report.buckets[0].automated_at_peak,
        report.buckets[0].max_agents
    );
    assert!(report.partial);
    assert!(report.as_of.is_some());
    assert!(report
        .pricing
        .as_ref()
        .unwrap()
        .latest_row_updated_at
        .is_none());
    assert_eq!(report.projects.len(), 3);
}

#[test]
fn nullable_pricing_bands_normalize_to_empty_typed_lists() {
    // If the official Go server emits an unbanded pricing rate as a nil slice,
    // JSON contains `bands: null`; rejecting it makes the Activity dashboard unavailable.
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/report-v6.json")).unwrap();
    let resolution = &mut value["pricing"]["models"]["model-alpha"]["resolutions"][0];
    resolution["bands"] = serde_json::Value::Null;
    resolution["application"]["bands"] = serde_json::Value::Null;

    let report = serde_json::from_value::<Report>(value)
        .expect("official nil pricing slices must decode as empty lists");
    let resolution = &report.pricing.unwrap().models["model-alpha"].resolutions[0];

    assert!(resolution.bands.is_empty());
    assert!(resolution.application.bands.is_empty());
}

#[test]
fn nullable_untimed_session_models_normalize_to_an_empty_typed_list() {
    // If an untimed session has no usage attribution, the official Go constructor leaves
    // its models slice nil; rejecting the resulting null makes the whole report unavailable.
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/report-v6.json")).unwrap();
    value["by_session"][2]["models"] = serde_json::Value::Null;

    let report = serde_json::from_value::<Report>(value)
        .expect("official nil session models must decode as an empty list");

    assert!(report.by_session[2].models.is_empty());
}

#[test]
fn bucket_input_tokens_decode_on_schema_v6() {
    // AgentsView added input_tokens to activity buckets without bumping schema_version.
    // If the decoder still treats that field as unknown, live reports fail before render.
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/report-v6.json")).unwrap();
    value["buckets"][0]["input_tokens"] = serde_json::json!(1200);
    value["buckets"][1]["input_tokens"] = serde_json::json!(800);

    let report = serde_json::from_value::<Report>(value)
        .expect("schema v6 reports with bucket input_tokens must decode");

    assert_eq!(report.buckets[0].input_tokens, 1200);
    assert_eq!(report.buckets[1].input_tokens, 800);
}

#[test]
fn unknown_contract_field_does_not_fail_decode() {
    // Additive same-version keys must leave the rest of the report usable. Collection of
    // unused paths happens at the HTTP decoder, not on a bare serde_json value.
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/report-v6.json")).unwrap();
    value["unexpected"] = serde_json::json!(true);

    let report = serde_json::from_value::<Report>(value)
        .expect("additive fields must not fail schema v6 decode");
    assert_eq!(report.schema_version, ACTIVITY_SCHEMA_VERSION);
}

#[test]
fn removed_v5_intervals_field_does_not_fail_v6_decode() {
    // A v6 payload that still carries the removed intervals key is an additive leftover,
    // not a mislabeled v5 report. Schema version still rejects an actual v5 body.
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/report-v6.json")).unwrap();
    value["intervals"] = serde_json::json!([]);

    serde_json::from_value::<Report>(value).expect("leftover intervals must not fail v6 decode");
}

#[test]
fn invalid_report_timezone_is_rejected_at_the_wire_boundary() {
    // If the report timezone remains an unchecked string, valid UTC instants can reach the
    // renderer without a reliable local-time interpretation.
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/report-v6.json")).unwrap();
    value["timezone"] = serde_json::json!("not/a-timezone");

    assert!(serde_json::from_value::<Report>(value).is_err());
}

#[test]
fn unknown_nested_field_does_not_fail_decode() {
    // Additive keys on a session row must not drop the rest of the report. Unknown closed
    // enum values remain a contract break because they change modeled behavior.
    let mut extra_field: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/report-v6.json")).unwrap();
    extra_field["by_session"][0]["unexpected"] = serde_json::json!(true);

    serde_json::from_value::<Report>(extra_field)
        .expect("additive session fields must not fail schema v6 decode");
}

#[test]
fn unknown_closed_enum_is_rejected() {
    // If a closed enum grows under schema v6, accepting it would make sorting and
    // timing-quality behavior silently incomplete.
    let mut new_enum: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/report-v6.json")).unwrap();
    new_enum["by_session"][0]["timing_quality"] = serde_json::json!("estimated");

    assert!(serde_json::from_value::<Report>(new_enum).is_err());
}

#[test]
fn nullable_metadata_arrays_normalize_to_empty_typed_lists() {
    // If AgentsView serializes a nil Go slice as null, filters must remain usable instead
    // of treating the valid empty response as malformed JSON.
    let projects: ProjectsResponse =
        serde_json::from_str(include_str!("fixtures/projects.json")).unwrap();
    let agents: AgentsResponse =
        serde_json::from_str(include_str!("fixtures/agents.json")).unwrap();
    let machines: herdr_agentsview::wire::MachinesResponse =
        serde_json::from_str(include_str!("fixtures/machines.json")).unwrap();

    assert!(projects.into_projects().is_empty());
    assert!(agents.into_agents().is_empty());
    assert!(machines.into_machines().is_empty());
}

#[test]
fn populated_metadata_preserves_names_and_counts() {
    // If metadata wrapper decoding drifts from the endpoint contract, the Activity
    // selectors would lose their labels or session counts while reports still load.
    let projects: ProjectsResponse = serde_json::from_value(serde_json::json!({
        "projects": [{"name": "project-alpha", "session_count": 2}]
    }))
    .unwrap();
    let agents: AgentsResponse = serde_json::from_value(serde_json::json!({
        "agents": [{"name": "codex", "session_count": 3}]
    }))
    .unwrap();

    assert_eq!(
        projects.into_projects(),
        vec![ProjectInfo {
            name: "project-alpha".to_owned(),
            session_count: 2,
        }]
    );
    assert_eq!(
        agents.into_agents(),
        vec![AgentInfo {
            name: "codex".to_owned(),
            session_count: 3,
        }]
    );
}

#[test]
fn report_selection_emits_only_supported_activity_filters() {
    // If query construction grows browser-only or speculative parameters, an otherwise
    // valid dashboard request can be rejected by the official Activity endpoint.
    let selection = ReportSelection::new(
        NaiveDate::from_ymd_opt(2026, 8, 8).unwrap(),
        "America/New_York".parse().unwrap(),
    )
    .with_project("project-alpha")
    .with_agent("codex")
    .with_machine("machine-alpha")
    .with_automation(Automation::Automated);

    assert_eq!(
        selection.query_pairs(),
        vec![
            ("preset", "day".to_owned()),
            ("date", "2026-08-08".to_owned()),
            ("timezone", "America/New_York".to_owned()),
            ("project", "project-alpha".to_owned()),
            ("agent", "codex".to_owned()),
            ("machine", "machine-alpha".to_owned()),
            ("automation", "automated".to_owned()),
        ]
    );
}
