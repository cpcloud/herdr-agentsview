use std::collections::BTreeMap;

use chrono::{DateTime, FixedOffset, NaiveDate};
use chrono_tz::Tz;
use serde::{Deserialize, Deserializer, Serialize};

pub const ACTIVITY_SCHEMA_VERSION: u32 = 5;

fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<Vec<T>>::deserialize(deserializer).map(Option::unwrap_or_default)
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Automation {
    All,
    Interactive,
    Automated,
}

impl Automation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Interactive => "interactive",
            Self::Automated => "automated",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReportSelection {
    pub date: NaiveDate,
    pub timezone: Tz,
    pub project: Option<String>,
    pub agent: Option<String>,
    pub machine: Option<String>,
    pub automation: Automation,
}

impl ReportSelection {
    pub fn new(date: NaiveDate, timezone: Tz) -> Self {
        Self {
            date,
            timezone,
            project: None,
            agent: None,
            machine: None,
            automation: Automation::All,
        }
    }

    pub fn with_project(mut self, project: impl Into<String>) -> Self {
        self.project = Some(project.into());
        self
    }

    pub fn with_agent(mut self, agent: impl Into<String>) -> Self {
        self.agent = Some(agent.into());
        self
    }

    pub fn with_machine(mut self, machine: impl Into<String>) -> Self {
        self.machine = Some(machine.into());
        self
    }

    pub fn with_automation(mut self, automation: Automation) -> Self {
        self.automation = automation;
        self
    }

    pub fn query_pairs(&self) -> Vec<(&'static str, String)> {
        let mut pairs = vec![
            ("preset", "day".to_owned()),
            ("date", self.date.format("%Y-%m-%d").to_string()),
            ("timezone", self.timezone.name().to_owned()),
        ];
        if let Some(project) = &self.project {
            pairs.push(("project", project.clone()));
        }
        if let Some(agent) = &self.agent {
            pairs.push(("agent", agent.clone()));
        }
        if let Some(machine) = &self.machine {
            pairs.push(("machine", machine.clone()));
        }
        pairs.push(("automation", self.automation.as_str().to_owned()));
        pairs
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Money {
    pub microdollars: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CostSource {
    Computed,
    Reported,
    Mixed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectResolution {
    Resolved,
    Unknown,
    Ambiguous,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectKind {
    GitRemote,
    MachineRoot,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TimingQuality {
    Timed,
    Untimed,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub schema_version: u32,
    pub pricing: Option<PricingBlock>,
    pub projects: BTreeMap<String, ProjectMapEntry>,
    pub timezone: Tz,
    pub range_start: DateTime<FixedOffset>,
    pub range_end: DateTime<FixedOffset>,
    pub bucket_unit: String,
    pub bucket_seconds: u64,
    pub bucket_count: usize,
    pub partial: bool,
    pub as_of: Option<DateTime<FixedOffset>>,
    pub effective_end: DateTime<FixedOffset>,
    pub elapsed_bucket_count: usize,
    pub buckets: Vec<Bucket>,
    pub peak: Peak,
    pub totals: Totals,
    pub by_project: Vec<KeyMinutes>,
    pub by_model: Vec<KeyMinutes>,
    pub by_agent: Vec<KeyMinutes>,
    pub by_session: Vec<SessionRow>,
    pub intervals: Vec<ReportInterval>,
}

impl Report {
    pub(crate) fn first_activity_bucket_index(&self) -> usize {
        let observed = if self.partial {
            self.elapsed_bucket_count.min(self.buckets.len())
        } else {
            self.buckets.len()
        };
        self.buckets
            .iter()
            .take(observed)
            .position(|bucket| {
                bucket
                    .interactive_at_peak
                    .saturating_add(bucket.automated_at_peak)
                    > 0
            })
            .unwrap_or(0)
    }

    pub(crate) fn observed_bucket_end(&self, bucket: &Bucket) -> DateTime<FixedOffset> {
        if self.partial {
            bucket.end.min(self.effective_end)
        } else {
            bucket.end
        }
    }

    pub(crate) fn bucket_is_future(&self, bucket: &Bucket) -> bool {
        self.partial && bucket.start >= self.effective_end
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PricingBlock {
    pub source: String,
    pub table_version: String,
    pub latest_row_updated_at: Option<DateTime<FixedOffset>>,
    pub custom_override_count: usize,
    pub effective_row_count: usize,
    pub digest: String,
    pub cost_source: CostSource,
    pub fallback: PricingFallback,
    pub models: BTreeMap<String, ModelPricingProvenance>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PricingFallback {
    pub used: bool,
    pub models: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelPricingProvenance {
    pub cost_source: CostSource,
    pub resolutions: Vec<EffectiveModelRate>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveModelRate {
    pub priced_model: String,
    pub matched_pattern: Option<String>,
    pub input_cost_per_mtok: Money,
    pub output_cost_per_mtok: Money,
    pub cache_write_cost_per_mtok: Money,
    pub cache_read_cost_per_mtok: Money,
    pub cost_source: CostSource,
    #[serde(deserialize_with = "deserialize_null_default")]
    pub bands: Vec<PricingBand>,
    pub application: PricingApplication,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PricingBand {
    pub above_input_tokens: u64,
    pub input_cost_per_mtok: Money,
    pub output_cost_per_mtok: Money,
    pub cache_write_cost_per_mtok: Money,
    pub cache_read_cost_per_mtok: Money,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PricingApplication {
    pub base_request_count: usize,
    pub aggregate_row_count: usize,
    #[serde(deserialize_with = "deserialize_null_default")]
    pub bands: Vec<AppliedPricingBand>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AppliedPricingBand {
    pub above_input_tokens: u64,
    pub request_count: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectIdentity {
    pub key: String,
    pub kind: ProjectKind,
    pub normalized_remote: Option<String>,
    pub root_key: Option<String>,
    pub repository_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectMapEntry {
    pub display_label: String,
    pub resolution: ProjectResolution,
    pub identity: Option<ProjectIdentity>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Bucket {
    pub start: DateTime<FixedOffset>,
    pub end: DateTime<FixedOffset>,
    pub max_agents: usize,
    pub agent_minutes: f64,
    pub output_tokens: u64,
    pub cost: Money,
    pub automated_at_peak: usize,
    pub interactive_at_peak: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReportInterval {
    pub session_id: String,
    pub start: DateTime<FixedOffset>,
    pub end: DateTime<FixedOffset>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Peak {
    pub agents: usize,
    pub at: Option<DateTime<FixedOffset>>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Totals {
    pub active_minutes: f64,
    pub idle_minutes: f64,
    pub agent_minutes: f64,
    pub sessions: usize,
    pub untimed_sessions: usize,
    pub distinct_projects: usize,
    pub distinct_models: usize,
    pub output_tokens: u64,
    pub cost: Money,
    pub automated_agent_minutes: f64,
    pub interactive_agent_minutes: f64,
    pub automated_cost: Money,
    pub interactive_cost: Money,
    pub automated_sessions: usize,
    pub interactive_sessions: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct KeyMinutes {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_key: Option<String>,
    pub key: String,
    pub agent_minutes: f64,
    pub cost: Money,
    pub automated_agent_minutes: f64,
    pub interactive_agent_minutes: f64,
    pub automated_cost: Money,
    pub interactive_cost: Money,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRow {
    pub session_id: String,
    pub project_key: String,
    pub title: String,
    pub project: String,
    pub agent: String,
    pub primary_model: String,
    #[serde(deserialize_with = "deserialize_null_default")]
    pub models: Vec<String>,
    pub agent_minutes: Option<f64>,
    pub cost: Money,
    pub output_tokens: u64,
    pub first_active: Option<DateTime<FixedOffset>>,
    pub last_active: Option<DateTime<FixedOffset>>,
    pub timing_quality: TimingQuality,
    pub is_automated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectInfo {
    pub name: String,
    pub session_count: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentInfo {
    pub name: String,
    pub session_count: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectsResponse {
    projects: Option<Vec<ProjectInfo>>,
}

impl ProjectsResponse {
    pub fn into_projects(self) -> Vec<ProjectInfo> {
        self.projects.unwrap_or_default()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentsResponse {
    agents: Option<Vec<AgentInfo>>,
}

impl AgentsResponse {
    pub fn into_agents(self) -> Vec<AgentInfo> {
        self.agents.unwrap_or_default()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MachinesResponse {
    machines: Option<Vec<String>>,
}

impl MachinesResponse {
    pub fn into_machines(self) -> Vec<String> {
        self.machines.unwrap_or_default()
    }
}
