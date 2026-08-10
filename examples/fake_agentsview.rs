use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context};
use chrono::{DateTime, FixedOffset, NaiveDate, TimeDelta, TimeZone, Utc};
use chrono_tz::Tz;
use clap::Parser;
use herdr_agentsview::wire::{AgentInfo, Bucket, Money, ProjectInfo, Report};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use url::Url;

const READY_REPORT: &str = include_str!("../tests/fixtures/report-v5.json");
const PROJECT_ALPHA_REPORT: &str = include_str!("../tests/fixtures/report-project-alpha-v5.json");
const AUTOMATED_REPORT: &str = include_str!("../tests/fixtures/report-automated-v5.json");
const EMPTY_REPORT: &str = include_str!("../tests/fixtures/report-empty-v5.json");
const MAX_REQUEST_BYTES: usize = 64 * 1024;

#[derive(Parser)]
#[command(about = "Isolated loopback AgentsView boundary for Activity development")]
struct Args {
    #[arg(long, value_name = "PATH")]
    config_out: PathBuf,

    #[arg(long, default_value_t = 0)]
    initial_delay_ms: u64,

    #[arg(long, default_value_t = 0)]
    refresh_delay_ms: u64,

    #[arg(long)]
    error_on_refresh: bool,
}

#[derive(Default)]
struct ServerState {
    report_count: AtomicUsize,
}

#[derive(Serialize)]
struct ProjectsBody {
    projects: Vec<ProjectInfo>,
}

#[derive(Serialize)]
struct AgentsBody {
    agents: Vec<AgentInfo>,
}

#[derive(Serialize)]
struct MachinesBody {
    machines: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReportScenario {
    Ready,
    ProjectAlpha,
    Automated,
    Empty,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("bind fake AgentsView to loopback")?;
    let address = listener.local_addr().context("read fake server address")?;
    write_config(&args.config_out, address.port())?;
    eprintln!(
        "fake AgentsView listening on http://{address}/; config written to {}",
        args.config_out.display()
    );

    let args = Arc::new(args);
    let state = Arc::new(ServerState::default());
    loop {
        let (stream, _) = listener.accept().await.context("accept fake request")?;
        let request_args = Arc::clone(&args);
        let request_state = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(error) = serve(stream, request_args, request_state).await {
                eprintln!("fake AgentsView request failed: {error:#}");
            }
        });
    }
}

async fn serve(
    mut stream: TcpStream,
    args: Arc<Args>,
    state: Arc<ServerState>,
) -> anyhow::Result<()> {
    let request = read_request(&mut stream).await?;
    let request_line = request
        .lines()
        .next()
        .context("request has no request line")?;
    let mut fields = request_line.split_whitespace();
    let method = fields.next().context("request has no method")?;
    let target = fields.next().context("request has no target")?;
    if method != "GET" {
        return write_json_response(
            &mut stream,
            "405 Method Not Allowed",
            r#"{"error":"method not allowed"}"#,
        )
        .await;
    }

    let url =
        Url::parse(&format!("http://127.0.0.1{target}")).context("parse fake request target")?;
    let query = url.query_pairs().into_owned().collect::<BTreeMap<_, _>>();
    let (status, body) = match url.path() {
        "/api/v1/activity/report" => {
            let ordinal = state.report_count.fetch_add(1, Ordering::SeqCst) + 1;
            let delay = if ordinal == 1 {
                args.initial_delay_ms
            } else {
                args.refresh_delay_ms
            };
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            if ordinal > 1 && args.error_on_refresh {
                (
                    "503 Service Unavailable",
                    r#"{"error":"simulated refresh failure"}"#.to_owned(),
                )
            } else {
                (
                    "200 OK",
                    serde_json::to_string(&scenario_report(&query)?)
                        .context("encode fake Activity report")?,
                )
            }
        }
        "/api/v1/projects" => (
            "200 OK",
            serde_json::to_string(&project_metadata()).context("encode fake project metadata")?,
        ),
        "/api/v1/agents" => (
            "200 OK",
            serde_json::to_string(&agent_metadata()).context("encode fake agent metadata")?,
        ),
        "/api/v1/machines" => (
            "200 OK",
            serde_json::to_string(&MachinesBody {
                machines: vec!["machine-alpha".to_owned(), "machine-beta".to_owned()],
            })
            .context("encode fake machine metadata")?,
        ),
        _ => ("404 Not Found", r#"{"error":"route not found"}"#.to_owned()),
    };
    write_json_response(&mut stream, status, &body).await
}

async fn read_request(stream: &mut TcpStream) -> anyhow::Result<String> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let count = stream.read(&mut chunk).await.context("read request")?;
        if count == 0 {
            bail!("request ended before headers completed");
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            return String::from_utf8(bytes).context("request headers are not UTF-8");
        }
        if bytes.len() > MAX_REQUEST_BYTES {
            bail!("request headers exceed {MAX_REQUEST_BYTES} bytes");
        }
    }
}

async fn write_json_response(
    stream: &mut TcpStream,
    status: &str,
    body: &str,
) -> anyhow::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(head.as_bytes())
        .await
        .context("write response headers")?;
    stream
        .write_all(body.as_bytes())
        .await
        .context("write response body")
}

fn scenario_report(query: &BTreeMap<String, String>) -> anyhow::Result<Report> {
    let fixture = match scenario_for_query(query) {
        ReportScenario::Ready => READY_REPORT,
        ReportScenario::ProjectAlpha => PROJECT_ALPHA_REPORT,
        ReportScenario::Automated => AUTOMATED_REPORT,
        ReportScenario::Empty => EMPTY_REPORT,
    };
    let mut report: Report =
        serde_json::from_str(fixture).context("decode committed Activity report scenario")?;
    align_report_date(&mut report, query.get("date"), query.get("timezone"))?;
    expand_report_to_local_day(&mut report)?;
    Ok(report)
}

fn scenario_for_query(query: &BTreeMap<String, String>) -> ReportScenario {
    // Metadata counts describe the unfiltered Ready fixture. Filtered responses choose whole
    // contract fixtures instead of rebuilding AgentsView aggregates; project-gamma and
    // incompatible combinations intentionally route to Empty so the demo can exercise that state.
    let project = query.get("project").map(String::as_str);
    let agent = query.get("agent").map(String::as_str);
    let machine = query.get("machine").map(String::as_str);
    let automation = query.get("automation").map_or("all", String::as_str);
    let has_filter =
        project.is_some() || agent.is_some() || machine.is_some() || automation != "all";
    if !has_filter {
        return ReportScenario::Ready;
    }

    let project_alpha = project.is_none_or(|value| value == "project-alpha")
        && agent.is_none_or(|value| value == "codex")
        && machine.is_none_or(|value| value == "machine-alpha")
        && matches!(automation, "all" | "interactive");
    if project_alpha {
        return ReportScenario::ProjectAlpha;
    }

    let automated = project.is_none_or(|value| value == "project-beta")
        && agent.is_none_or(|value| value == "reviewer")
        && machine.is_none_or(|value| value == "machine-beta")
        && matches!(automation, "all" | "automated");
    if automated {
        ReportScenario::Automated
    } else {
        ReportScenario::Empty
    }
}

fn align_report_date(
    report: &mut Report,
    requested: Option<&String>,
    timezone: Option<&String>,
) -> anyhow::Result<()> {
    let source_timezone = report.timezone;
    let timezone_name = timezone.map_or(source_timezone.name(), String::as_str);
    let timezone = timezone_name
        .parse::<Tz>()
        .with_context(|| format!("parse fake report timezone {timezone_name}"))?;
    let delta = requested.map_or(Ok(TimeDelta::zero()), |requested| {
        let requested = NaiveDate::parse_from_str(requested, "%Y-%m-%d")
            .context("parse requested fake report date")?;
        let local_time = report.range_start.with_timezone(&source_timezone).time();
        let local_start = requested.and_time(local_time);
        let shifted_start = timezone
            .from_local_datetime(&local_start)
            .single()
            .context("resolve fake report date in timezone")?;
        Ok::<_, anyhow::Error>(
            shifted_start.with_timezone(&Utc) - report.range_start.with_timezone(&Utc),
        )
    })?;
    report.timezone = timezone;
    report.range_start = shift_timestamp(report.range_start, delta);
    report.range_end = shift_timestamp(report.range_end, delta);
    report.effective_end = shift_timestamp(report.effective_end, delta);
    report.as_of = report
        .as_of
        .take()
        .map(|value| shift_timestamp(value, delta));
    report.peak.at = report
        .peak
        .at
        .take()
        .map(|value| shift_timestamp(value, delta));
    for bucket in &mut report.buckets {
        bucket.start = shift_timestamp(bucket.start, delta);
        bucket.end = shift_timestamp(bucket.end, delta);
    }
    for session in &mut report.by_session {
        session.first_active = session
            .first_active
            .take()
            .map(|value| shift_timestamp(value, delta));
        session.last_active = session
            .last_active
            .take()
            .map(|value| shift_timestamp(value, delta));
    }
    for interval in &mut report.intervals {
        interval.start = shift_timestamp(interval.start, delta);
        interval.end = shift_timestamp(interval.end, delta);
    }
    Ok(())
}

fn expand_report_to_local_day(report: &mut Report) -> anyhow::Result<()> {
    if report.bucket_seconds == 0 {
        bail!("fake report bucket_seconds must be greater than zero");
    }
    let date = report
        .range_start
        .with_timezone(&report.timezone)
        .date_naive();
    let next_date = date
        .succ_opt()
        .context("fake report date has no successor")?;
    let midnight = date.and_hms_opt(0, 0, 0).context("construct midnight")?;
    let next_midnight = next_date
        .and_hms_opt(0, 0, 0)
        .context("construct next midnight")?;
    let day_start = report
        .timezone
        .from_local_datetime(&midnight)
        .single()
        .context("resolve fake report day start")?
        .with_timezone(&Utc);
    let day_end = report
        .timezone
        .from_local_datetime(&next_midnight)
        .single()
        .context("resolve fake report day end")?
        .with_timezone(&Utc);
    let bucket_seconds = i64::try_from(report.bucket_seconds)
        .context("fake report bucket_seconds exceeds supported range")?;
    let day_seconds = (day_end - day_start).num_seconds();
    if day_seconds % bucket_seconds != 0 {
        bail!("fake report day is not divisible by its bucket size");
    }

    let existing = std::mem::take(&mut report.buckets);
    let template = existing
        .first()
        .cloned()
        .context("fake report has no bucket template")?;
    let mut by_start = existing
        .into_iter()
        .map(|bucket| (bucket.start.with_timezone(&Utc), bucket))
        .collect::<BTreeMap<_, _>>();
    let bucket_span = TimeDelta::seconds(bucket_seconds);
    let mut cursor = day_start;
    let mut buckets = Vec::with_capacity((day_seconds / bucket_seconds) as usize);
    while cursor < day_end {
        let end = cursor + bucket_span;
        let mut bucket = by_start
            .remove(&cursor)
            .unwrap_or_else(|| empty_bucket(&template));
        bucket.start = cursor.fixed_offset();
        bucket.end = end.fixed_offset();
        buckets.push(bucket);
        cursor = end;
    }
    if !by_start.is_empty() {
        bail!("fake report contains buckets outside its selected day");
    }

    report.range_start = day_start.fixed_offset();
    report.range_end = day_end.fixed_offset();
    report.bucket_count = buckets.len();
    report.elapsed_bucket_count = if report.partial {
        let effective_end = report.effective_end.with_timezone(&Utc);
        buckets.partition_point(|bucket| bucket.start.with_timezone(&Utc) < effective_end)
    } else {
        buckets.len()
    };
    report.buckets = buckets;
    Ok(())
}

fn empty_bucket(template: &Bucket) -> Bucket {
    let mut bucket = template.clone();
    bucket.max_agents = 0;
    bucket.agent_minutes = 0.0;
    bucket.output_tokens = 0;
    bucket.cost = Money { microdollars: 0 };
    bucket.automated_at_peak = 0;
    bucket.interactive_at_peak = 0;
    bucket
}

fn shift_timestamp(value: DateTime<FixedOffset>, delta: TimeDelta) -> DateTime<FixedOffset> {
    (value.with_timezone(&Utc) + delta).fixed_offset()
}

fn project_metadata() -> ProjectsBody {
    ProjectsBody {
        projects: vec![
            ProjectInfo {
                name: "project-alpha".to_owned(),
                session_count: 1,
            },
            ProjectInfo {
                name: "project-beta".to_owned(),
                session_count: 1,
            },
            ProjectInfo {
                name: "project-gamma".to_owned(),
                session_count: 1,
            },
        ],
    }
}

fn agent_metadata() -> AgentsBody {
    AgentsBody {
        agents: vec![
            AgentInfo {
                name: "codex".to_owned(),
                session_count: 2,
            },
            AgentInfo {
                name: "reviewer".to_owned(),
                session_count: 1,
            },
        ],
    }
}

fn write_config(path: &Path, port: u16) -> anyhow::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create config directory {}", parent.display()))?;
    }
    let temp = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name()
            .context("--config-out must name a file")?
            .to_string_lossy(),
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)
        .with_context(|| format!("create temporary config {}", temp.display()))?;
    let result = (|| {
        writeln!(
            file,
            "api_base_url = \"http://127.0.0.1:{port}/\"\nrequest_timeout_seconds = 2\nrefresh_interval_seconds = 3600\ntimezone = \"America/New_York\""
        )
        .context("write fake Activity config")?;
        file.sync_all().context("sync fake Activity config")?;
        fs::rename(&temp, path)
            .with_context(|| format!("replace fake Activity config {}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::{NaiveTime, TimeDelta};

    use super::{scenario_for_query, scenario_report, ReportScenario};

    #[test]
    fn default_query_selects_the_populated_contract_fixture() {
        // If the API's explicit `all` value is mistaken for a filter, the standalone dashboard
        // opens empty and cannot exercise its populated layout.
        let query = BTreeMap::from([("automation".to_owned(), "all".to_owned())]);

        let report = scenario_report(&query).unwrap();

        assert_eq!(scenario_for_query(&query), ReportScenario::Ready);
        assert_eq!(report.totals.sessions, 3);
        assert_eq!(report.by_session.len(), 3);
    }

    #[test]
    fn filters_select_matching_ready_and_empty_scenarios() {
        // If query routing chooses the wrong response scenario, the standalone filter flow can
        // show an incompatible session class or miss the empty-selection state entirely.
        let project = BTreeMap::from([("project".to_owned(), "project-alpha".to_owned())]);
        let agent = BTreeMap::from([("agent".to_owned(), "codex".to_owned())]);
        let machine = BTreeMap::from([("machine".to_owned(), "machine-beta".to_owned())]);
        let automated = BTreeMap::from([("automation".to_owned(), "automated".to_owned())]);
        let incompatible = BTreeMap::from([
            ("project".to_owned(), "project-alpha".to_owned()),
            ("automation".to_owned(), "automated".to_owned()),
        ]);

        assert_eq!(
            scenario_report(&project).unwrap().by_session[0].session_id,
            "session-alpha"
        );
        assert_eq!(
            scenario_report(&automated).unwrap().by_session[0].session_id,
            "session-beta"
        );
        assert_eq!(scenario_for_query(&agent), ReportScenario::ProjectAlpha);
        assert_eq!(scenario_for_query(&machine), ReportScenario::Automated);
        assert_eq!(scenario_report(&incompatible).unwrap().totals.sessions, 0);
    }

    #[test]
    fn demo_reports_cover_the_full_local_day() {
        // If the fake serves the compact contract fixture unchanged, the one-command demo
        // compresses its concurrency axis to a 30-minute slice instead of the selected day.
        let report = scenario_report(&BTreeMap::from([(
            "automation".to_owned(),
            "all".to_owned(),
        )]))
        .unwrap();
        let local_start = report.range_start.with_timezone(&report.timezone);
        let local_end = report.range_end.with_timezone(&report.timezone);
        let range_seconds = (report.range_end - report.range_start).num_seconds();

        assert_eq!(local_start.time(), NaiveTime::MIN);
        assert_eq!(local_end.time(), NaiveTime::MIN);
        assert_eq!(
            local_end.date_naive(),
            local_start.date_naive().succ_opt().unwrap()
        );
        assert_eq!(
            range_seconds,
            report.bucket_count as i64 * report.bucket_seconds as i64
        );
        assert_eq!(report.buckets.len(), report.bucket_count);
        assert!(report.buckets.iter().any(|bucket| bucket.max_agents > 0));
    }

    #[test]
    fn requested_date_uses_the_timezone_offset_for_that_day() {
        // If date alignment retains the fixture offset, a response across a DST boundary claims
        // one timezone while returning instants for another local day.
        let report = scenario_report(&BTreeMap::from([
            ("automation".to_owned(), "all".to_owned()),
            ("date".to_owned(), "2026-11-01".to_owned()),
            ("timezone".to_owned(), "America/New_York".to_owned()),
        ]))
        .unwrap();
        let local_start = report.range_start.with_timezone(&report.timezone);
        let local_end = report.range_end.with_timezone(&report.timezone);

        assert_eq!(local_start.time(), NaiveTime::MIN);
        assert_eq!(local_end.time(), NaiveTime::MIN);
        assert_eq!(
            local_end.date_naive(),
            local_start.date_naive().succ_opt().unwrap()
        );
        assert_eq!(report.range_end - report.range_start, TimeDelta::hours(25));
    }

    #[test]
    fn timezone_only_request_keeps_official_utc_timestamp_shape() {
        // If changing only display timezone rewrites timestamp offsets, the fake no longer
        // matches AgentsView's UTC-instants-plus-timezone contract.
        let report = scenario_report(&BTreeMap::from([(
            "timezone".to_owned(),
            "Asia/Tokyo".to_owned(),
        )]))
        .unwrap();
        let local_start = report.range_start.with_timezone(&report.timezone);

        assert_eq!(report.timezone.name(), "Asia/Tokyo");
        assert_eq!(report.range_start.offset().local_minus_utc(), 0);
        assert_eq!(report.range_end.offset().local_minus_utc(), 0);
        assert_eq!(local_start.time(), NaiveTime::MIN);
    }
}
