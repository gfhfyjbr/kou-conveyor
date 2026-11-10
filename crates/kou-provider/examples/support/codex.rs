use kou_domain::Json;
use kou_provider::{
    Error, Result,
    codex::{Client, Config},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, io::Write};

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Input {
    mode: String,
    environment: BTreeMap<String, String>,
    access_token: String,
    #[serde(rename = "AccountID")]
    account_id: String,
    #[serde(rename = "BaseURL")]
    base_url: String,
    max_attempts: Option<usize>,
    auth_body: Option<String>,
    file_mode: Option<u32>,
}
pub(super) fn run(raw: &Json) -> Result<Value> {
    let input: Input = raw.decode().map_err(|error| Error(error.to_string()))?;
    if input.mode == "environment" {
        if input.environment.get("HOME").is_none_or(String::is_empty) {
            return Err(Error("oracle environment needs a synthetic HOME".into()));
        }
        return Ok(
            match Config::from_environment(
                |name| input.environment.get(name).cloned().unwrap_or_default(),
                None,
            ) {
                Ok(config) => {
                    json!({"Config":{"AccessToken":config.access_token,"AccountID":config.account_id,"AuthFile":config.auth_file.map(|path|path.to_string_lossy().into_owned()).unwrap_or_default()},"Error":""})
                }
                Err(error) => json!({"Config":Value::Null,"Error":error.to_string()}),
            },
        );
    }
    let mut config = Config {
        access_token: input.access_token,
        account_id: input.account_id,
        base_url: input.base_url,
        max_attempts: input.max_attempts,
        ..Config::default()
    };
    let directory = tempfile::tempdir().map_err(|error| Error(error.to_string()))?;
    let mut file = None;
    if let Some(body) = input.auth_body {
        let mut auth = tempfile::NamedTempFile::new_in(directory.path())
            .map_err(|error| Error(error.to_string()))?;
        auth.write_all(body.as_bytes())
            .map_err(|error| Error(error.to_string()))?;
        #[cfg(unix)]
        if let Some(mode) = input.file_mode {
            use std::os::unix::fs::PermissionsExt;
            auth.as_file()
                .set_permissions(std::fs::Permissions::from_mode(mode & 0o777))
                .map_err(|error| Error(error.to_string()))?;
        }
        config.auth_file = Some(auth.path().into());
        file = Some(auth);
    }
    let result = Client::new(config);
    drop(file);
    Ok(json!({"Error":result.err().map(|error|error.to_string()).unwrap_or_default()}))
}
