// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use chrono::NaiveDate;
use herdr_agentsview::wire::{
    AgentInfo, AgentsResponse, Automation, ProjectInfo, ProjectsResponse, Report, ReportSelection,
    TimingQuality, ACTIVITY_SCHEMA_VERSION,
};

// Contract recorded from kenn-io/agentsview revision
// 14992d32da35a40c666eaaf3fa54a3d59f54d25f.
#[test]
fn report_v5_fixture_decodes_exact_contract() {
    // If AgentsView changes the versioned response shape without coordinated client work,
    // the dashboard must fail at the boundary instead of rendering invented defaults.
    let report: Report = serde_json::from_str(include_str!("fixtures/report-v5.json"))
        .expect("official schema-v5 fixture must decode");

    assert_eq!(report.schema_version, ACTIVITY_SCHEMA_VERSION);
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
        serde_json::from_str(include_str!("fixtures/report-v5.json")).unwrap();
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
        serde_json::from_str(include_str!("fixtures/report-v5.json")).unwrap();
    value["by_session"][2]["models"] = serde_json::Value::Null;

    let report = serde_json::from_value::<Report>(value)
        .expect("official nil session models must decode as an empty list");

    assert!(report.by_session[2].models.is_empty());
}

#[test]
fn unknown_contract_field_is_rejected() {
    // If a same-version response grows silently, strict decoding must force an explicit
    // compatibility decision rather than dropping data the UI may need.
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/report-v5.json")).unwrap();
    value["unexpected"] = serde_json::json!(true);

    assert!(serde_json::from_value::<Report>(value).is_err());
}

#[test]
fn invalid_report_timezone_is_rejected_at_the_wire_boundary() {
    // If the report timezone remains an unchecked string, valid UTC instants can reach the
    // renderer without a reliable local-time interpretation.
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/report-v5.json")).unwrap();
    value["timezone"] = serde_json::json!("not/a-timezone");

    assert!(serde_json::from_value::<Report>(value).is_err());
}

#[test]
fn unknown_nested_field_and_closed_enum_are_rejected() {
    // If a session row or closed enum changes under schema v5, accepting it would make
    // sorting and timing-quality behavior silently incomplete.
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/report-v5.json")).unwrap();
    let mut extra_field = fixture.clone();
    extra_field["by_session"][0]["unexpected"] = serde_json::json!(true);
    let mut new_enum = fixture;
    new_enum["by_session"][0]["timing_quality"] = serde_json::json!("estimated");

    assert!(serde_json::from_value::<Report>(extra_field).is_err());
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
