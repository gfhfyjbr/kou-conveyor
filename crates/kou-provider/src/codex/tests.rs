use super::*;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::json;
#[cfg(unix)]
use std::fs;
use std::{collections::BTreeMap, io::Write, time::Duration};

fn token(account: &str, expires: i64) -> String {
    let claims =
        json!({"exp":expires,"https://api.openai.com/auth":{"chatgpt_account_id":account}});
    format!(
        "e30.{}.signature",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap())
    )
}
fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}
fn auth_file(dir: &Path, body: &str) -> PathBuf {
    let (mut file, path) = tempfile::NamedTempFile::new_in(dir)
        .unwrap()
        .keep()
        .unwrap();
    file.write_all(body.as_bytes()).unwrap();
    path
}
#[test]
fn credential_claims_opaque_tokens_expiry_and_header_admission() {
    let config = Config {
        access_token: token("account", 1_700_000_001),
        ..Config::default()
    };
    let credentials = config.credentials(now()).unwrap();
    assert_eq!(credentials.account_id, "account");
    assert_eq!(credentials.access_token, config.access_token);
    for (access_token, account_id, expected) in [
        ("opaque".into(), " account ", None),
        ("opaque".into(), "", Some("account ID must be set")),
        ("sk-secret".into(), "account", Some("not an API key")),
        (
            "bad\r\nheader".into(),
            "account",
            Some("invalid header value"),
        ),
        ("bad secret".into(), "account", Some("invalid header value")),
        (
            "opaque".into(),
            "bad\r\nheader",
            Some("account ID must be set"),
        ),
        (token("account", 1_700_000_000), "", Some("expired")),
        (token("account", -1), "", Some("expired")),
        (token("account", 0), "other", Some("does not match")),
        (
            "one.!.three".into(),
            "account",
            Some("invalid Codex access token claims"),
        ),
        ("e30.bnVsbA.signature".into(), "account", None),
        (
            "e30.bnVsbA.signature".into(),
            "",
            Some("account ID must be set"),
        ),
    ] {
        let config = Config {
            access_token: access_token.clone(),
            account_id: account_id.into(),
            ..Config::default()
        };
        match expected {
            None => assert_eq!(config.credentials(now()).unwrap().account_id, "account"),
            Some(expected) => {
                let error = config.credentials(now()).err().unwrap().to_string();
                assert!(error.contains(expected), "{error}");
                assert!(
                    !error.contains(&access_token),
                    "error disclosed synthetic token"
                );
            }
        }
    }
    let config = Config {
        access_token: "x".repeat(CREDENTIAL_BYTES + 1),
        account_id: "account".into(),
        ..Config::default()
    };
    assert!(
        config
            .credentials(now())
            .err()
            .unwrap()
            .0
            .contains("credential-byte limit")
    );
}
#[test]
fn jwt_schema_errors_and_noncanonical_trailing_bits_match_go_raw_url_decode() {
    for body in [
        "{broken",
        "[]",
        r#"{"exp":"1"}"#,
        r#"{"exp":1.2}"#,
        r#"{"exp":1,"exp":2}"#,
        r#"{"https://api.openai.com/auth":{"chatgpt_account_id":5}}"#,
    ] {
        assert!(
            token_claims(&format!("header.{}.sig", URL_SAFE_NO_PAD.encode(body))).is_err(),
            "{body}"
        );
    }
    for body in [
        "null",
        r#"{"exp":null,"https://api.openai.com/auth":null}"#,
        r#"{"unknown":1e999}"#,
    ] {
        assert_eq!(
            token_claims(&format!("header.{}.sig", URL_SAFE_NO_PAD.encode(body))).unwrap(),
            (String::new(), 0)
        );
    }
    // '{}' is e30. Go's non-strict RawURLEncoding accepts e31's unused low bits.
    assert_eq!(
        token_claims("header.e31.signature").unwrap(),
        (String::new(), 0)
    );
}
#[test]
fn subscription_url_accepts_only_official_or_literal_loopback() {
    for value in [
        "",
        BASE_URL,
        " https://chatgpt.com/backend-api/codex/ ",
        "http://127.0.0.1:1234/custom",
        "http://127.0.0.2",
        "https://[::1]:1234/v1",
        "http://[::ffff:127.0.0.1]:1234",
        "http://127.0.0.1?",
        "http://127.0.0.1#",
    ] {
        assert!(validate_base_url(value).is_ok(), "{value}");
    }
    for value in [
        "https://api.openai.com/v1",
        "http://chatgpt.com/backend-api/codex",
        "https://chatgpt.com:443/backend-api/codex",
        "http://localhost",
        "http://0.0.0.0",
        "http://192.168.1.1",
        "http://user:pass@127.0.0.1",
        "http://@127.0.0.1",
        "http://127.0.0.1?q=key",
        "http://127.0.0.1/#fragment",
        "ftp://127.0.0.1",
        "http://2130706433",
        "http://0x7f000001",
        "http://127.1",
        "http://127.0.00.1",
        "http://%31%32%37.0.0.1",
        "http://[::1%25en0]",
        "http://127.0.0.1\\external",
    ] {
        assert!(validate_base_url(value).is_err(), "{value}");
    }
}
#[test]
fn environment_source_selection_is_injected_and_does_not_read_secrets() {
    for (env, expected) in [
        (
            vec![("HOME", "/tmp/home"), ("OPENAI_API_KEY", "sk-never-used")],
            "/tmp/home/.codex/auth.json",
        ),
        (
            vec![("HOME", "/tmp/home"), ("CODEX_HOME", " /tmp/codex ")],
            "/tmp/codex/auth.json",
        ),
        (
            vec![("OPENAI_CODEX_AUTH_FILE", " /missing/auth.json ")],
            "/missing/auth.json",
        ),
    ] {
        let env: BTreeMap<_, _> = env.into_iter().collect();
        let config = Config::from_environment(
            |name| env.get(name).copied().unwrap_or_default().into(),
            None,
        )
        .unwrap();
        assert_eq!(config.auth_file.as_deref(), Some(Path::new(expected)));
        assert!(config.access_token.is_empty());
    }
    let config = Config::from_environment(
        |name| {
            if name == "OPENAI_CODEX_ACCESS_TOKEN" {
                " token ".into()
            } else {
                String::new()
            }
        },
        None,
    )
    .unwrap();
    assert_eq!(config.access_token, "token");
    assert!(config.auth_file.is_none());
    assert!(
        Config::from_environment(
            |name| match name {
                "OPENAI_CODEX_ACCESS_TOKEN" => "token",
                "OPENAI_CODEX_AUTH_FILE" => "file",
                _ => "",
            }
            .into(),
            None
        )
        .is_err()
    );
    assert!(Config::from_environment(|_| String::new(), None).is_err());
    assert_eq!(
        Config::from_environment(|_| String::new(), Some(Path::new("/tmp/fallback")))
            .unwrap()
            .auth_file
            .as_deref(),
        Some(Path::new("/tmp/fallback/.codex/auth.json"))
    );
}
#[test]
fn private_auth_file_is_read_once_with_size_schema_and_mode_checks() {
    let dir = tempfile::tempdir().unwrap();
    let file=auth_file(dir.path(),&json!({"auth_mode":"chatgpt","tokens":{"access_token":token("file-account",0),"refresh_token":"unused"}}).to_string());
    let config = Config {
        auth_file: Some(file),
        ..Config::default()
    };
    assert_eq!(
        config.credentials(now()).unwrap().account_id,
        "file-account"
    );
    for body in [
        r#"{"auth_mode":"apikey","tokens":{"access_token":"private"}}"#,
        "{not-json",
        "[]",
        r#"{"tokens":{"access_token":42}}"#,
        r#"{"tokens":{},"tokens":{}}"#,
    ] {
        let path = auth_file(dir.path(), body);
        let config = Config {
            auth_file: Some(path),
            ..Config::default()
        };
        assert!(config.credentials(now()).is_err());
    }
    for body in [
        "null",
        r#"{"auth_mode":null,"tokens":null}"#,
        r#"{"tokens":{"access_token":null,"account_id":null}}"#,
    ] {
        let config = Config {
            auth_file: Some(auth_file(dir.path(), body)),
            ..Config::default()
        };
        assert!(
            config
                .credentials(now())
                .err()
                .unwrap()
                .0
                .contains("access token must be set")
        );
    }
    let config = Config {
        auth_file: Some(auth_file(dir.path(), &"x".repeat(CREDENTIAL_BYTES + 1))),
        ..Config::default()
    };
    assert!(
        config
            .credentials(now())
            .err()
            .unwrap()
            .0
            .contains("invalid Codex auth file")
    );
    let config = Config {
        auth_file: Some(dir.path().into()),
        ..Config::default()
    };
    assert!(
        config
            .credentials(now())
            .err()
            .unwrap()
            .0
            .contains("regular file")
    );
    let config = Config {
        access_token: "token".into(),
        auth_file: Some(dir.path().into()),
        ..Config::default()
    };
    assert!(
        config
            .credentials(now())
            .err()
            .unwrap()
            .0
            .contains("cannot be combined")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = auth_file(
            dir.path(),
            r#"{"tokens":{"access_token":"opaque","account_id":"account"}}"#,
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            Config {
                auth_file: Some(path),
                ..Config::default()
            }
            .credentials(now())
            .err()
            .unwrap()
            .0
            .contains("private permissions")
        );
        let path = dir.path().join("fifo");
        assert!(
            std::process::Command::new("mkfifo")
                .arg("-m")
                .arg("600")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Config {
                auth_file: Some(path),
                ..Config::default()
            }
            .credentials(now())
            .err()
            .unwrap()
            .0
            .contains("regular file")
        );
    }
}
