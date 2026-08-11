// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use secrecy::ExposeSecret;
use tempfile::TempDir;

use super::{ConfigEnvironment, PluginConfig, RuntimeAuth};

const VALID_CONFIG: &str = r#"
api_base_url = "https://activity.example.invalid/base"
request_timeout_seconds = 10
refresh_interval_seconds = 300
timezone = "America/New_York"
"#;

struct ConfigFixture {
    _temp: TempDir,
    environment: ConfigEnvironment,
}

impl ConfigFixture {
    fn with_config(contents: &str) -> Self {
        let temp = TempDir::new().unwrap();
        let config_dir = temp.path().join("plugin-config");
        fs::create_dir(&config_dir).unwrap();
        fs::write(config_dir.join("config.toml"), contents).unwrap();
        Self {
            _temp: temp,
            environment: ConfigEnvironment {
                plugin_config_dir: Some(config_dir),
                tz: None,
                token: None,
                token_file: None,
                os_timezone: Ok("Etc/UTC".to_owned()),
            },
        }
    }
}

fn generated_credential() -> String {
    format!("unit-credential-{}", std::process::id())
}

#[test]
fn valid_config_loads_from_the_managed_plugin_directory() {
    // If the loader falls back to a second config location, Herdr and Home Manager can
    // update one file while the dashboard silently reads another.
    let fixture = ConfigFixture::with_config(VALID_CONFIG);

    let config = PluginConfig::load_from(&fixture.environment).unwrap();

    assert_eq!(
        config.api_base_url.as_str(),
        "https://activity.example.invalid/base/"
    );
    assert_eq!(config.request_timeout, Some(Duration::from_secs(10)));
    assert_eq!(config.refresh_interval, Duration::from_secs(300));
    assert_eq!(config.timezone.name(), "America/New_York");
    assert!(config.auth.is_none());
}

#[test]
fn omitted_request_timeout_leaves_activity_requests_unbounded() {
    // If an omitted timeout is silently replaced with a short client deadline, a report
    // that the AgentsView browser UI would keep waiting for fails in the dashboard.
    let fixture = ConfigFixture::with_config(
        r#"
api_base_url = "https://activity.example.invalid/base"
refresh_interval_seconds = 300
timezone = "America/New_York"
"#,
    );

    let config = PluginConfig::load_from(&fixture.environment).unwrap();

    assert!(config.request_timeout.is_none());
}

#[test]
fn https_config_preserves_each_resolved_runtime_auth_source() {
    // If the loader validates a runtime credential but drops it while constructing the
    // config, every authenticated server request becomes an unexplained 401.
    let credential = generated_credential();
    let mut direct = ConfigFixture::with_config(VALID_CONFIG);
    direct.environment.token = Some(OsString::from(&credential));

    let direct_config = PluginConfig::load_from(&direct.environment).unwrap();
    assert_eq!(
        direct_config.auth.as_ref().unwrap().expose_secret(),
        &credential
    );

    let mut from_file = ConfigFixture::with_config(VALID_CONFIG);
    let token_file = from_file._temp.path().join("runtime-token");
    fs::write(&token_file, format!("{credential}\n")).unwrap();
    from_file.environment.token_file = Some(token_file);

    let file_config = PluginConfig::load_from(&from_file.environment).unwrap();
    assert_eq!(
        file_config.auth.as_ref().unwrap().expose_secret(),
        &credential
    );
}

#[test]
fn missing_or_invalid_plugin_config_directory_is_actionable() {
    // If Herdr did not inject a usable plugin directory, reading a default localhost
    // endpoint could reach the wrong server instead of failing closed.
    let missing = ConfigEnvironment {
        plugin_config_dir: None,
        tz: None,
        token: None,
        token_file: None,
        os_timezone: Ok("Etc/UTC".to_owned()),
    };
    let relative = ConfigEnvironment {
        plugin_config_dir: Some(PathBuf::from("relative")),
        ..missing_environment()
    };
    let temp = TempDir::new().unwrap();
    let file = temp.path().join("not-a-directory");
    fs::write(&file, "not a directory").unwrap();
    let not_directory = ConfigEnvironment {
        plugin_config_dir: Some(file),
        ..missing_environment()
    };

    let missing_error = PluginConfig::load_from(&missing).unwrap_err();
    let relative_error = PluginConfig::load_from(&relative).unwrap_err();
    let file_error = PluginConfig::load_from(&not_directory).unwrap_err();

    assert!(format!("{missing_error:#}").contains("HERDR_PLUGIN_CONFIG_DIR"));
    assert!(format!("{relative_error:#}").contains("absolute"));
    assert!(format!("{file_error:#}").contains("directory"));
}

#[test]
fn missing_or_unknown_config_is_not_replaced_with_defaults() {
    // If a missing or misspelled setting silently falls back, the dashboard can query an
    // unintended endpoint or refresh at a surprising cadence.
    let temp = TempDir::new().unwrap();
    let config_dir = temp.path().join("plugin-config");
    fs::create_dir(&config_dir).unwrap();
    let missing = ConfigEnvironment {
        plugin_config_dir: Some(config_dir),
        ..missing_environment()
    };
    let unknown = ConfigFixture::with_config(&format!("{VALID_CONFIG}\nunknown = true\n"));

    let missing_error = PluginConfig::load_from(&missing).unwrap_err();
    let unknown_error = PluginConfig::load_from(&unknown.environment).unwrap_err();

    assert!(format!("{missing_error:#}").contains("config.toml"));
    assert!(format!("{unknown_error:#}").contains("parse"));
}

#[test]
fn malformed_config_errors_do_not_echo_source_lines() {
    // If a credential is mistakenly placed in the TOML file, parse diagnostics must not
    // copy the offending line into terminal output or a persistent Herdr pane.
    let credential = generated_credential();
    let fixture = ConfigFixture::with_config(&format!("{VALID_CONFIG}\ntoken = {credential:?}\n"));

    let error = PluginConfig::load_from(&fixture.environment).unwrap_err();
    let rendered = format!("{error:#}");

    assert!(rendered.contains("parse Activity config"));
    assert!(!rendered.contains(&credential), "{rendered}");
}

#[test]
fn unsafe_base_url_components_are_rejected() {
    // If URL credentials, queries, or fragments survive validation, fixed endpoint joins
    // can send requests to an ambiguous origin or leak credentials into diagnostics.
    for url in [
        "https://placeholder!@activity.example.invalid/",
        "https://activity.example.invalid/?scope=all",
        "https://activity.example.invalid/#activity",
    ] {
        let fixture = ConfigFixture::with_config(&config_with(url, Some("Etc/UTC"), 10, 300));

        assert!(
            PluginConfig::load_from(&fixture.environment).is_err(),
            "{url}"
        );
    }
}

#[test]
fn plain_http_requires_a_literal_loopback_address_and_no_credentials() {
    // If HTTP validation accepts hostnames or credentials, DNS rebinding or a copied
    // production URL can expose a bearer token on the network.
    for url in [
        "http://localhost:9000/",
        "http://192.0.2.1:9000/",
        "http://activity.example.invalid/",
    ] {
        let fixture = ConfigFixture::with_config(&config_with(url, Some("Etc/UTC"), 10, 300));
        assert!(
            PluginConfig::load_from(&fixture.environment).is_err(),
            "{url}"
        );
    }

    for url in ["http://127.42.0.1:9000/", "http://[::1]:9000/"] {
        let fixture = ConfigFixture::with_config(&config_with(url, Some("Etc/UTC"), 10, 300));
        assert!(
            PluginConfig::load_from(&fixture.environment).is_ok(),
            "{url}"
        );
    }

    let mut credentialed = ConfigFixture::with_config(&config_with(
        "http://127.0.0.1:9000/",
        Some("Etc/UTC"),
        10,
        300,
    ));
    credentialed.environment.token = Some(OsString::from(generated_credential()));
    let error = PluginConfig::load_from(&credentialed.environment).unwrap_err();
    assert!(format!("{error:#}").contains("HTTPS"));
}

#[test]
fn zero_request_or_refresh_duration_is_rejected() {
    // If either duration reaches zero, requests can fail immediately or the refresh loop
    // can spin continuously.
    let zero_timeout =
        ConfigFixture::with_config(&config_with("https://example.invalid/", None, 0, 300));
    let zero_refresh =
        ConfigFixture::with_config(&config_with("https://example.invalid/", None, 10, 0));

    assert!(format!(
        "{:#}",
        PluginConfig::load_from(&zero_timeout.environment).unwrap_err()
    )
    .contains("request_timeout_seconds"));
    assert!(format!(
        "{:#}",
        PluginConfig::load_from(&zero_refresh.environment).unwrap_err()
    )
    .contains("refresh_interval_seconds"));
}

#[test]
fn timezone_resolution_is_explicit_then_tz_then_operating_system() {
    // If precedence drifts, the same selected date can describe a different calendar day
    // from the one shown in the terminal.
    let explicit = ConfigFixture::with_config(VALID_CONFIG);
    let mut from_tz =
        ConfigFixture::with_config(&config_with("https://example.invalid/", None, 10, 300));
    from_tz.environment.tz = Some(OsString::from("Europe/London"));
    from_tz.environment.os_timezone = Ok("Asia/Tokyo".to_owned());
    let mut from_os =
        ConfigFixture::with_config(&config_with("https://example.invalid/", None, 10, 300));
    from_os.environment.tz = Some(OsString::from("not/a-zone"));
    from_os.environment.os_timezone = Ok("Asia/Tokyo".to_owned());

    assert_eq!(
        PluginConfig::load_from(&explicit.environment)
            .unwrap()
            .timezone
            .name(),
        "America/New_York"
    );
    assert_eq!(
        PluginConfig::load_from(&from_tz.environment)
            .unwrap()
            .timezone
            .name(),
        "Europe/London"
    );
    assert_eq!(
        PluginConfig::load_from(&from_os.environment)
            .unwrap()
            .timezone
            .name(),
        "Asia/Tokyo"
    );
}

#[test]
fn invalid_explicit_or_absent_timezone_is_actionable() {
    // If no valid IANA zone is available, silently using UTC would move the user's date
    // window without any visible explanation.
    let invalid = ConfigFixture::with_config(&config_with(
        "https://example.invalid/",
        Some("not/a-zone"),
        10,
        300,
    ));
    let mut absent =
        ConfigFixture::with_config(&config_with("https://example.invalid/", None, 10, 300));
    absent.environment.os_timezone = Err("timezone unavailable".to_owned());

    assert!(format!(
        "{:#}",
        PluginConfig::load_from(&invalid.environment).unwrap_err()
    )
    .contains("timezone"));
    assert!(format!(
        "{:#}",
        PluginConfig::load_from(&absent.environment).unwrap_err()
    )
    .contains("timezone"));
}

#[test]
fn token_sources_are_mutually_exclusive() {
    // If both sources are accepted, operators cannot know which credential was sent.
    let temp = TempDir::new().unwrap();
    let token_file = temp.path().join("token");
    fs::write(&token_file, generated_credential()).unwrap();
    let environment = ConfigEnvironment {
        token: Some(OsString::from(generated_credential())),
        token_file: Some(token_file),
        ..missing_environment()
    };

    let error = RuntimeAuth::resolve(&environment).unwrap_err();

    assert!(format!("{error:#}").contains("both"));
}

#[test]
fn token_file_removes_exactly_one_line_ending() {
    // If trimming is broader than one transport newline, a malformed credential can be
    // changed silently; if it trims nothing, the header becomes invalid.
    let temp = TempDir::new().unwrap();
    let token_file = temp.path().join("token");
    let credential = generated_credential();
    fs::write(&token_file, format!("{credential}\r\n")).unwrap();
    let environment = ConfigEnvironment {
        token_file: Some(token_file.clone()),
        ..missing_environment()
    };

    let auth = RuntimeAuth::resolve(&environment).unwrap().unwrap();
    assert_eq!(auth.expose_secret(), &credential);

    fs::write(&token_file, format!("{credential}\n\n")).unwrap();
    assert!(RuntimeAuth::resolve(&environment).is_err());
}

#[test]
fn empty_or_control_bearing_tokens_are_rejected_without_disclosure() {
    // If invalid credential bytes are echoed or accepted, they can leak to logs or create
    // malformed Authorization headers.
    for (value, classification) in [
        (String::new(), "must not be empty"),
        (
            format!("{}\nextra", generated_credential()),
            "control character",
        ),
    ] {
        let environment = ConfigEnvironment {
            token: Some(OsString::from(&value)),
            ..missing_environment()
        };
        let error = RuntimeAuth::resolve(&environment).unwrap_err();
        let rendered = format!("{error:#}");
        assert!(rendered.contains(classification));
        if !value.is_empty() {
            assert!(!rendered.contains(&value));
        }
    }
}

#[test]
fn secret_debug_and_configuration_errors_are_redacted() {
    // If a credential-bearing value gains an unsafe Debug or error path, terminal errors
    // and test failures can persist the secret.
    let credential = generated_credential();
    let environment = ConfigEnvironment {
        token: Some(OsString::from(&credential)),
        ..missing_environment()
    };
    let auth = RuntimeAuth::resolve(&environment).unwrap().unwrap();
    let debug = format!("{auth:?}");
    assert!(!debug.contains(&credential));

    let mut fixture = ConfigFixture::with_config(&config_with(
        "http://127.0.0.1:9000/",
        Some("Etc/UTC"),
        10,
        300,
    ));
    fixture.environment.token = Some(OsString::from(&credential));
    let error = PluginConfig::load_from(&fixture.environment).unwrap_err();
    assert!(!format!("{error:#}").contains(&credential));
}

fn missing_environment() -> ConfigEnvironment {
    ConfigEnvironment {
        plugin_config_dir: None,
        tz: None,
        token: None,
        token_file: None,
        os_timezone: Ok("Etc/UTC".to_owned()),
    }
}

fn config_with(
    url: &str,
    timezone: Option<&str>,
    request_timeout_seconds: u64,
    refresh_interval_seconds: u64,
) -> String {
    let timezone = timezone
        .map(|value| format!("timezone = {value:?}\n"))
        .unwrap_or_default();
    format!(
        "api_base_url = {url:?}\nrequest_timeout_seconds = {request_timeout_seconds}\nrefresh_interval_seconds = {refresh_interval_seconds}\n{timezone}"
    )
}
