use super::*;
use peridot_common::{peridot_home_dir, user_home_dir};

pub(crate) async fn run_login_command(provider: AuthProvider, output: OutputFormat) -> Result<()> {
    if provider == AuthProvider::OpenaiOauth {
        return run_openai_oauth_login(output).await;
    }
    let env_var = provider
        .api_key_env_var()
        .with_context(|| format!("{} does not use API-key login", provider.id()))?;
    let api_key =
        std::env::var(env_var).with_context(|| format!("{env_var} is required for login"))?;
    if provider == AuthProvider::OpenrouterApi {
        let path = upsert_managed_env_var(env_var, &api_key)?;
        return print_json_or_text_result(
            serde_json::json!({
                "provider": provider.id(),
                "path": path,
                "stored": true,
                "env": env_var
            }),
            format!("stored {env_var} in Peridot local environment"),
            output,
        );
    }
    let path = auth_file(provider)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(&serde_json::json!({
        "provider": provider.id(),
        "api_key": api_key
    }))?;
    fs::write(&path, content)?;
    set_private_permissions(&path)?;
    print_json_or_text_result(
        serde_json::json!({"provider": provider.id(), "path": path, "stored": true}),
        format!("stored credentials for {}", provider.id()),
        output,
    )
}

pub(crate) fn run_logout_command(provider: AuthProvider, output: OutputFormat) -> Result<()> {
    if provider == AuthProvider::OpenrouterApi {
        let path = env_store_file()?;
        let removed = remove_managed_env_var("OPENROUTER_API_KEY")?;
        return print_json_or_text_result(
            serde_json::json!({"provider": provider.id(), "path": path, "removed": removed}),
            format!("removed credentials for {}: {removed}", provider.id()),
            output,
        );
    }
    let path = auth_file(provider)?;
    let removed = if path.exists() {
        fs::remove_file(&path)?;
        true
    } else {
        false
    };
    print_json_or_text_result(
        serde_json::json!({"provider": provider.id(), "path": path, "removed": removed}),
        format!("removed credentials for {}: {removed}", provider.id()),
        output,
    )
}

pub(crate) fn read_stored_api_key(provider: AuthProvider) -> Result<Option<String>> {
    let path = auth_file(provider)?;
    if !path.exists() {
        return Ok(None);
    }
    let value = serde_json::from_str::<Value>(&fs::read_to_string(path)?)?;
    Ok(value
        .get("api_key")
        .and_then(Value::as_str)
        .map(str::to_string))
}

pub(crate) fn read_managed_env_var(key: &str) -> Result<Option<String>> {
    let path = env_store_file()?;
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path)?;
    Ok(parse_local_env_value(&content, key))
}

pub(crate) fn run_env_command(command: &EnvCommand, output: OutputFormat) -> Result<()> {
    match command {
        EnvCommand::Set { key, value } => {
            validate_env_key(key)?;
            let value = match value {
                Some(value) => value.clone(),
                None => read_stdin_env_value(key)?,
            };
            let path = upsert_managed_env_var(key, &value)?;
            print_json_or_text_result(
                serde_json::json!({"key": key, "path": path, "stored": true}),
                format!("stored {key} in Peridot local environment"),
                output,
            )
        }
        EnvCommand::Get { key } => {
            validate_env_key(key)?;
            let value = read_managed_env_var(key)?;
            if output == OutputFormat::Json {
                print_json_or_text_result(
                    serde_json::json!({"key": key, "value": value}),
                    String::new(),
                    output,
                )
            } else if let Some(value) = value {
                println!("{value}");
                Ok(())
            } else {
                anyhow::bail!("{key} is not stored in Peridot local environment");
            }
        }
        EnvCommand::List => {
            let keys = list_managed_env_keys()?;
            print_json_or_text_result(serde_json::json!({"keys": keys}), keys.join("\n"), output)
        }
        EnvCommand::Unset { key } => {
            validate_env_key(key)?;
            let path = env_store_file()?;
            let removed = remove_managed_env_var(key)?;
            print_json_or_text_result(
                serde_json::json!({"key": key, "path": path, "removed": removed}),
                format!("removed {key} from Peridot local environment: {removed}"),
                output,
            )
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct OpenAiOAuthCredentials {
    pub access_token: String,
    pub account_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct OpenAiOAuthIdentity {
    pub account_id: Option<String>,
    pub chatgpt_plan_type: Option<String>,
    pub email: Option<String>,
}

pub(crate) async fn read_stored_openai_oauth_credentials() -> Result<Option<OpenAiOAuthCredentials>>
{
    let path = auth_file(AuthProvider::OpenaiOauth)?;
    if !path.exists() {
        return Ok(None);
    }
    let mut value = serde_json::from_str::<Value>(&fs::read_to_string(&path)?)?;
    if openai_oauth_token_expires_within(&value, 300)
        && let Some(refreshed) = refresh_openai_oauth_token(&value).await?
    {
        value = refreshed;
        fs::write(&path, serde_json::to_string_pretty(&value)?)?;
        set_private_permissions(&path)?;
    }
    let Some(access_token) = value
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return Ok(None);
    };
    let identity = openai_oauth_access_token_identity(&access_token);
    let account_id = value
        .get("account_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(identity.account_id);
    Ok(Some(OpenAiOAuthCredentials {
        access_token,
        account_id,
    }))
}

pub(crate) fn openai_oauth_access_token_identity(access_token: &str) -> OpenAiOAuthIdentity {
    let Some(payload) = decode_openai_oauth_jwt_payload(access_token) else {
        return OpenAiOAuthIdentity::default();
    };
    let auth = payload
        .get("https://api.openai.com/auth")
        .unwrap_or(&Value::Null);
    let profile = payload
        .get("https://api.openai.com/profile")
        .unwrap_or(&Value::Null);
    OpenAiOAuthIdentity {
        account_id: auth
            .get("chatgpt_account_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        chatgpt_plan_type: auth
            .get("chatgpt_plan_type")
            .and_then(Value::as_str)
            .map(str::to_string),
        email: profile
            .get("email")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

fn decode_openai_oauth_jwt_payload(access_token: &str) -> Option<Value> {
    let payload = access_token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    serde_json::from_slice::<Value>(&bytes).ok()
}

pub(super) fn openai_oauth_token_expires_within(token: &Value, leeway_seconds: u64) -> bool {
    let Some(obtained_at) = token.get("obtained_at_unix").and_then(Value::as_u64) else {
        return false;
    };
    let Some(expires_in) = token.get("expires_in").and_then(Value::as_u64) else {
        return false;
    };
    let expires_at = obtained_at.saturating_add(expires_in);
    let now = unix_timestamp();
    now.saturating_add(leeway_seconds) >= expires_at
}

pub(super) async fn refresh_openai_oauth_token(token: &Value) -> Result<Option<Value>> {
    let Some(refresh_token) = token.get("refresh_token").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(client_id) = token.get("client_id").and_then(Value::as_str) else {
        return Ok(None);
    };
    let mut refreshed = exchange_openai_oauth_refresh_token(client_id, refresh_token)
        .await
        .with_context(|| "failed to refresh OpenAI OAuth token")?;
    let has_new_refresh_token = refreshed
        .get("refresh_token")
        .and_then(Value::as_str)
        .is_some();
    if let Some(object) = refreshed.as_object_mut() {
        enrich_openai_oauth_token_object(object);
        object.insert(
            "provider".to_string(),
            Value::String(AuthProvider::OpenaiOauth.id().to_string()),
        );
        object.insert(
            "client_id".to_string(),
            Value::String(client_id.to_string()),
        );
        if !has_new_refresh_token {
            object.insert(
                "refresh_token".to_string(),
                Value::String(refresh_token.to_string()),
            );
        }
        if let Some(redirect_uri) = token.get("redirect_uri").and_then(Value::as_str) {
            object.insert(
                "redirect_uri".to_string(),
                Value::String(redirect_uri.to_string()),
            );
        }
        object.insert(
            "obtained_at_unix".to_string(),
            Value::Number(serde_json::Number::from(unix_timestamp())),
        );
    }
    Ok(Some(refreshed))
}

pub(super) fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

const OPENAI_CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OPENAI_CODEX_OAUTH_PORT: u16 = 1455;
const OPENAI_CODEX_REDIRECT_PATH: &str = "/auth/callback";

pub(super) async fn run_openai_oauth_login(output: OutputFormat) -> Result<()> {
    let client_id =
        std::env::var("OPENAI_OAUTH_CLIENT_ID").unwrap_or_else(|_| OPENAI_CODEX_CLIENT_ID.into());
    let port = std::env::var("PERIDOT_OAUTH_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(OPENAI_CODEX_OAUTH_PORT);
    let scope = std::env::var("OPENAI_OAUTH_SCOPE")
        .unwrap_or_else(|_| "openid profile email offline_access".to_string());
    let originator =
        std::env::var("OPENAI_OAUTH_ORIGINATOR").unwrap_or_else(|_| "peridot".to_string());
    let redirect_uri = format!("http://localhost:{port}{OPENAI_CODEX_REDIRECT_PATH}");
    let state = random_urlsafe(32);
    let code_verifier = random_urlsafe(64);
    let code_challenge = pkce_challenge(&code_verifier);
    let auth_url = openai_oauth_authorize_url(
        &client_id,
        &redirect_uri,
        &scope,
        &state,
        &code_challenge,
        &originator,
    );

    if output == OutputFormat::Text {
        println!("Open this URL to authorize Peridot:\n{auth_url}");
        if open_browser(&auth_url) {
            println!("Opened browser; waiting for OAuth callback on {redirect_uri}");
        } else {
            println!("Could not open a browser automatically; paste the URL into your browser.");
        }
    }

    let code = wait_for_oauth_code(port, &state)?;
    let mut token = exchange_openai_oauth_code(&client_id, &redirect_uri, &code_verifier, &code)
        .await
        .with_context(|| "failed to exchange OpenAI OAuth authorization code")?;
    if let Some(object) = token.as_object_mut() {
        enrich_openai_oauth_token_object(object);
        object.insert(
            "provider".to_string(),
            Value::String(AuthProvider::OpenaiOauth.id().to_string()),
        );
        object.insert("client_id".to_string(), Value::String(client_id));
        object.insert("redirect_uri".to_string(), Value::String(redirect_uri));
        object.insert("originator".to_string(), Value::String(originator));
        object.insert(
            "obtained_at_unix".to_string(),
            Value::Number(serde_json::Number::from(
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
            )),
        );
    }

    let path = auth_file(AuthProvider::OpenaiOauth)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_string_pretty(&token)?)?;
    set_private_permissions(&path)?;
    print_json_or_text_result(
        serde_json::json!({
            "provider": AuthProvider::OpenaiOauth.id(),
            "path": path,
            "stored": true,
            "token_type": token.get("token_type").and_then(Value::as_str),
            "account_id": token.get("account_id").and_then(Value::as_str),
            "chatgpt_plan_type": token.get("chatgpt_plan_type").and_then(Value::as_str)
        }),
        format!("stored credentials for {}", AuthProvider::OpenaiOauth.id()),
        output,
    )
}

fn enrich_openai_oauth_token_object(object: &mut serde_json::Map<String, Value>) {
    let Some(access_token) = object.get("access_token").and_then(Value::as_str) else {
        return;
    };
    let identity = openai_oauth_access_token_identity(access_token);
    if let Some(account_id) = identity.account_id {
        object.insert("account_id".to_string(), Value::String(account_id));
    }
    if let Some(plan_type) = identity.chatgpt_plan_type {
        object.insert("chatgpt_plan_type".to_string(), Value::String(plan_type));
    }
    if let Some(email) = identity.email {
        object.insert("email".to_string(), Value::String(email));
    }
}

pub(super) fn openai_oauth_authorize_url(
    client_id: &str,
    redirect_uri: &str,
    scope: &str,
    state: &str,
    code_challenge: &str,
    originator: &str,
) -> String {
    format!(
        "https://auth.openai.com/oauth/authorize?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&code_challenge={}&code_challenge_method=S256&id_token_add_organizations=true&codex_cli_simplified_flow=true&originator={}",
        url_encode(client_id),
        url_encode(redirect_uri),
        url_encode(scope),
        url_encode(state),
        url_encode(code_challenge),
        url_encode(originator)
    )
}

pub(super) async fn exchange_openai_oauth_code(
    client_id: &str,
    redirect_uri: &str,
    code_verifier: &str,
    code: &str,
) -> Result<Value> {
    let response = reqwest::Client::new()
        .post("https://auth.openai.com/oauth/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", client_id),
            ("redirect_uri", redirect_uri),
            ("code_verifier", code_verifier),
            ("code", code),
        ])
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("OpenAI OAuth token exchange returned {status}: {body}");
    }
    Ok(serde_json::from_str(&body)?)
}

pub(super) async fn exchange_openai_oauth_refresh_token(
    client_id: &str,
    refresh_token: &str,
) -> Result<Value> {
    let response = reqwest::Client::new()
        .post("https://auth.openai.com/oauth/token")
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("OpenAI OAuth token refresh returned {status}: {body}");
    }
    Ok(serde_json::from_str(&body)?)
}

pub(super) fn wait_for_oauth_code(port: u16, expected_state: &str) -> Result<String> {
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => Some(listener),
        Err(error) => {
            eprintln!(
                "Could not bind local OAuth callback port {port}: {error}. Paste the redirect URL or authorization code:"
            );
            None
        }
    };
    if listener.is_none() {
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        return parse_authorization_input(&input, expected_state);
    }
    let listener = listener.unwrap();
    listener.set_nonblocking(true)?;
    // Earlier versions also spawned a background stdin reader so the
    // user could paste the redirect URL as a fallback. That reader
    // outlived the listener path: once the OAuth callback arrived
    // and this function returned, the still-blocked `read_line`
    // would silently swallow the user's *next* terminal input (the
    // setup wizard's model-picker prompt). On Windows that surfaced
    // as "I have to press Enter twice to register my choice", with
    // the first keystroke actually selecting the prompt's default.
    // Paste-fallback is only meaningful when the listener fails to
    // bind, and that path is already handled above, so the reader
    // is gone here.
    let deadline = SystemTime::now() + Duration::from_secs(300);
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_read_timeout(Some(Duration::from_secs(5)))?;
                let mut buffer = [0_u8; 8192];
                let size = stream.read(&mut buffer)?;
                let request = String::from_utf8_lossy(&buffer[..size]);
                let result = parse_oauth_callback(&request, expected_state);
                let body = if result.is_ok() {
                    "Peridot login complete. You can close this window."
                } else {
                    "Peridot login failed. Return to the terminal for details."
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes())?;
                return result;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if SystemTime::now() >= deadline {
                    anyhow::bail!(
                        "timed out waiting for OpenAI OAuth callback or pasted redirect URL"
                    );
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(error).with_context(|| "failed to accept OAuth callback"),
        }
    }
}

pub(super) fn parse_authorization_input(input: &str, expected_state: &str) -> Result<String> {
    let value = input.trim();
    if value.is_empty() {
        anyhow::bail!("authorization code cannot be empty");
    }
    if let Ok(url) = reqwest::Url::parse(value)
        && let Some(query) = url.query()
    {
        let params = parse_query(query)?;
        if let Some(state) = params.get("state")
            && state != expected_state
        {
            anyhow::bail!("OpenAI OAuth state mismatch");
        }
        if let Some(code) = params.get("code") {
            return Ok(code.clone());
        }
    }
    if let Some((code, state)) = value.split_once('#') {
        if !state.is_empty() && state != expected_state {
            anyhow::bail!("OpenAI OAuth state mismatch");
        }
        if !code.is_empty() {
            return Ok(code.to_string());
        }
    }
    if value.contains("code=") {
        let query = value
            .split_once('?')
            .map(|(_, query)| query)
            .unwrap_or(value);
        let params = parse_query(query)?;
        if let Some(state) = params.get("state")
            && state != expected_state
        {
            anyhow::bail!("OpenAI OAuth state mismatch");
        }
        if let Some(code) = params.get("code") {
            return Ok(code.clone());
        }
    }
    Ok(value.to_string())
}

pub(super) fn parse_oauth_callback(request: &str, expected_state: &str) -> Result<String> {
    let target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .with_context(|| "invalid OAuth callback request")?;
    let query = target
        .split_once('?')
        .map(|(_, query)| query)
        .unwrap_or_default();
    let params = parse_query(query)?;
    if let Some(error) = params.get("error") {
        anyhow::bail!("OpenAI OAuth error: {error}");
    }
    let state = params
        .get("state")
        .with_context(|| "OpenAI OAuth callback omitted state")?;
    if state != expected_state {
        anyhow::bail!("OpenAI OAuth state mismatch");
    }
    params
        .get("code")
        .cloned()
        .with_context(|| "OpenAI OAuth callback omitted code")
}

pub(super) fn parse_query(query: &str) -> Result<HashMap<String, String>> {
    let mut params = HashMap::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        params.insert(percent_decode(key)?, percent_decode(value)?);
    }
    Ok(params)
}

pub(super) fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub(super) fn random_urlsafe(bytes: usize) -> String {
    let mut random = vec![0_u8; bytes];
    for chunk in random.chunks_mut(32) {
        let sample: [u8; 32] = rand::random();
        let len = chunk.len();
        chunk.copy_from_slice(&sample[..len]);
    }
    URL_SAFE_NO_PAD.encode(random)
}

#[cfg(target_os = "macos")]
pub(super) fn open_browser(url: &str) -> bool {
    Command::new("open")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

#[cfg(target_os = "windows")]
pub(super) fn open_browser(url: &str) -> bool {
    // `cmd /C start "" "<url>"`. The URL has to live inside its own
    // pair of double quotes because cmd treats `&` as a command
    // separator — OAuth URLs are nothing but `&`-joined params, so
    // without the quoting the browser opens whatever sits before the
    // first `&` (which is what every Windows user reports as "wrong
    // URL"). Rust's normal `args()` lets CreateProcess quote each
    // arg, but cmd's *internal* parser ignores that quoting and
    // re-splits the raw command line, so we have to assemble the
    // line ourselves via `raw_arg`.
    use std::os::windows::process::CommandExt;
    let quoted = format!(r#"/C start "" "{}""#, url.replace('"', "%22"));
    Command::new("cmd")
        .raw_arg(quoted)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(super) fn open_browser(url: &str) -> bool {
    Command::new("xdg-open")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

pub(super) fn url_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

pub(super) fn percent_decode(value: &str) -> Result<String> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut iter = value.as_bytes().iter().copied();
    while let Some(byte) = iter.next() {
        match byte {
            b'+' => bytes.push(b' '),
            b'%' => {
                let high = iter.next().with_context(|| "incomplete percent escape")?;
                let low = iter.next().with_context(|| "incomplete percent escape")?;
                let hex = [high, low];
                let decoded = u8::from_str_radix(std::str::from_utf8(&hex)?, 16)
                    .with_context(|| "invalid percent escape")?;
                bytes.push(decoded);
            }
            _ => bytes.push(byte),
        }
    }
    Ok(String::from_utf8(bytes)?)
}

pub(super) fn auth_file(provider: AuthProvider) -> Result<PathBuf> {
    let home = user_home_dir().with_context(|| "HOME or USERPROFILE is required")?;
    Ok(home
        .join(".peridot/auth")
        .join(format!("{}.json", provider.id())))
}

pub(super) fn env_store_file() -> Result<PathBuf> {
    Ok(peridot_home()?.join("env"))
}

pub(super) fn peridot_home() -> Result<PathBuf> {
    peridot_home_dir().with_context(|| "HOME or USERPROFILE is required")
}

pub(super) fn upsert_managed_env_var(key: &str, value: &str) -> Result<PathBuf> {
    validate_env_key(key)?;
    let path = env_store_file()?;
    upsert_env_var_file(&path, key, value)
}

pub(super) fn upsert_env_var_file(path: &Path, key: &str, value: &str) -> Result<PathBuf> {
    validate_env_key(key)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let existing = fs::read_to_string(path).unwrap_or_default();
    let mut found = false;
    let mut lines = Vec::new();
    for line in existing.lines() {
        let trimmed = line.trim_start();
        if let Some((candidate, _)) = env_assignment(trimmed)
            && candidate == key
        {
            lines.push(format!("export {key}={}", quote_env_value(value)));
            found = true;
            continue;
        }
        lines.push(line.to_string());
    }
    if !found {
        lines.push(format!("export {key}={}", quote_env_value(value)));
    }
    let mut content = lines.join("\n");
    content.push('\n');
    fs::write(path, content)?;
    set_private_permissions(path)?;
    Ok(path.to_path_buf())
}

pub(super) fn remove_managed_env_var(key: &str) -> Result<bool> {
    validate_env_key(key)?;
    let path = env_store_file()?;
    remove_env_var_file(&path, key)
}

pub(super) fn remove_env_var_file(path: &Path, key: &str) -> Result<bool> {
    validate_env_key(key)?;
    if !path.exists() {
        return Ok(false);
    }
    let existing = fs::read_to_string(path)?;
    let mut removed = false;
    let lines = existing
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            let should_remove = env_assignment(trimmed)
                .map(|(candidate, _)| candidate == key)
                .unwrap_or(false);
            if should_remove {
                removed = true;
            }
            !should_remove
        })
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut content = lines.join("\n");
    if !content.is_empty() {
        content.push('\n');
    }
    fs::write(path, content)?;
    set_private_permissions(path)?;
    Ok(removed)
}

pub(super) fn list_managed_env_keys() -> Result<Vec<String>> {
    let path = env_store_file()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut keys = fs::read_to_string(path)?
        .lines()
        .filter_map(|line| env_assignment(line.trim()).map(|(key, _)| key.to_string()))
        .filter(|key| validate_env_key(key).is_ok())
        .collect::<Vec<_>>();
    keys.sort();
    keys.dedup();
    Ok(keys)
}

pub(super) fn parse_local_env_value(content: &str, key: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        let Some((candidate, value)) = env_assignment(trimmed) else {
            continue;
        };
        if candidate != key {
            continue;
        }
        return Some(unquote_env_value(value.trim()));
    }
    None
}

fn env_assignment(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let assignment = trimmed
        .strip_prefix("export ")
        .map(str::trim_start)
        .unwrap_or(trimmed);
    let (key, value) = assignment.split_once('=')?;
    Some((key.trim(), value))
}

pub(super) fn validate_env_key(key: &str) -> Result<()> {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        anyhow::bail!("environment variable name cannot be empty");
    };
    if !(first == '_' || first.is_ascii_alphabetic())
        || !chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
    {
        anyhow::bail!("{key} is not a valid environment variable name");
    }
    Ok(())
}

fn read_stdin_env_value(key: &str) -> Result<String> {
    let mut value = String::new();
    std::io::stdin()
        .read_to_string(&mut value)
        .with_context(|| format!("failed to read {key} from stdin"))?;
    let value = value.trim_end_matches(['\r', '\n']).to_string();
    if value.is_empty() {
        anyhow::bail!("provide a value argument or pipe {key} on stdin");
    }
    Ok(value)
}

pub(super) fn quote_env_value(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | ':'))
    {
        return value.to_string();
    }
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

pub(super) fn unquote_env_value(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        let inner = &trimmed[1..trimmed.len() - 1];
        let mut result = String::new();
        let mut chars = inner.chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                if let Some(next) = chars.next() {
                    result.push(next);
                }
            } else {
                result.push(ch);
            }
        }
        result
    } else {
        trimmed.to_string()
    }
}

#[cfg(unix)]
pub(super) fn set_private_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn set_private_permissions(_path: &Path) -> Result<()> {
    Ok(())
}
