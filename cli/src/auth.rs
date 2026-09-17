use std::{
    fs,
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use serde_json::{Value, json};
use url::Url;
use uuid::Uuid;

use crate::{ApiClient, Cli, validate_origin};

const DEFAULT_SERVER: &str = "http://127.0.0.1:3000/";

#[derive(Debug, Subcommand)]
pub(crate) enum AuthCommand {
    /// Validate a token and save the login for this server.
    Login,
    /// Verify the current credentials and show the authenticated profile.
    Status,
    /// Remove the saved login without touching any token source.
    Logout,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TokenSource {
    ExplicitToken,
    ExplicitFile,
    ExplicitStdin,
    EnvironmentToken,
    EnvironmentFile,
    SavedToken,
    SavedFile,
    None,
}

impl TokenSource {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::ExplicitToken => "explicit-token",
            Self::ExplicitFile => "explicit-file",
            Self::ExplicitStdin => "explicit-stdin",
            Self::EnvironmentToken => "environment-token",
            Self::EnvironmentFile => "environment-file",
            Self::SavedToken => "saved-token",
            Self::SavedFile => "saved-file",
            Self::None => "none",
        }
    }
}

pub(crate) struct ResolvedAuth {
    pub(crate) server: Url,
    pub(crate) token: Option<String>,
    pub(crate) source: TokenSource,
}

struct SavedAuth {
    server: Url,
    token: Option<String>,
    token_file: Option<PathBuf>,
}

pub(crate) fn resolve(cli: &Cli) -> Result<ResolvedAuth> {
    let explicit = explicit_token(cli)?;
    let explicit_server = cli.server.clone().map(validate_origin).transpose()?;
    let environment_server = if explicit_server.is_some() && explicit.is_some() {
        None
    } else {
        environment_server()?
    };
    let environment_credentials_allowed = explicit_server
        .as_ref()
        .zip(environment_server.as_ref())
        .is_none_or(|(explicit, environment)| explicit.origin() == environment.origin());
    let (server, saved) = match explicit_server.or(environment_server) {
        Some(server) => (server, None),
        None => {
            let saved = load_saved()?;
            let server = saved
                .as_ref()
                .map(|saved| saved.server.clone())
                .unwrap_or_else(default_server);
            (server, Some(saved))
        }
    };

    let (token, source) = if let Some((token, source)) = explicit {
        (Some(token), source)
    } else {
        if environment_credentials_allowed {
            if let Some(token) = environment_token()? {
                return Ok(ResolvedAuth {
                    server,
                    token: Some(token),
                    source: TokenSource::EnvironmentToken,
                });
            }
            if let Some(token) = environment_file_token()? {
                return Ok(ResolvedAuth {
                    server,
                    token: Some(token),
                    source: TokenSource::EnvironmentFile,
                });
            }
        }
        let saved = match saved {
            Some(saved) => saved,
            None => load_saved()?,
        };
        let Some(saved) = saved else {
            return Ok(ResolvedAuth {
                server,
                token: None,
                source: TokenSource::None,
            });
        };
        saved_token_for_origin(saved, &server)?
    };
    Ok(ResolvedAuth {
        server,
        token,
        source,
    })
}

pub(crate) async fn run(cli: &Cli, command: &AuthCommand) -> Result<Value> {
    match command {
        AuthCommand::Login => login(cli).await,
        AuthCommand::Status => status(cli).await,
        AuthCommand::Logout => logout(),
    }
}

async fn login(cli: &Cli) -> Result<Value> {
    let explicit = explicit_token(cli)?;
    let explicit_server = cli.server.clone().map(validate_origin).transpose()?;
    let environment_server = if explicit_server.is_some() && explicit.is_some() {
        None
    } else {
        environment_server()?
    };
    let environment_file = environment_file_path();
    let environment_credentials_bound_to_other_origin = explicit_server
        .as_ref()
        .zip(environment_server.as_ref())
        .is_some_and(|(explicit, environment)| explicit.origin() != environment.origin());
    let environment_token = if explicit.is_none() && !environment_credentials_bound_to_other_origin
    {
        environment_token()?
    } else {
        None
    };
    let environment_file = (!environment_credentials_bound_to_other_origin)
        .then_some(environment_file)
        .flatten();
    let interactive =
        explicit.is_none() && environment_token.is_none() && environment_file.is_none();
    if interactive && (!io::stdin().is_terminal() || !io::stderr().is_terminal()) {
        bail!("interactive login requires a terminal; use --token-stdin or --token-file");
    }
    let saved = if cli.server.is_none() && environment_server.is_none() {
        load_saved()?
    } else {
        None
    };
    let server = if let Some(server) = explicit_server {
        server
    } else if let Some(server) = environment_server {
        server
    } else if interactive {
        prompt_server(
            saved
                .as_ref()
                .map(|saved| saved.server.clone())
                .unwrap_or_else(default_server),
        )?
    } else {
        saved
            .as_ref()
            .map(|saved| saved.server.clone())
            .unwrap_or_else(default_server)
    };

    let (token, record) = if let Some((token, source)) = explicit {
        let record = match source {
            TokenSource::ExplicitFile => {
                let Some(path) = cli.token_file.as_deref() else {
                    bail!("explicit token file source is missing its path");
                };
                SavedAuth {
                    server: server.clone(),
                    token: None,
                    token_file: Some(stored_token_file(path)?),
                }
            }
            TokenSource::ExplicitStdin | TokenSource::ExplicitToken => SavedAuth {
                server: server.clone(),
                token: Some(token.clone()),
                token_file: None,
            },
            _ => bail!("invalid explicit token source"),
        };
        (token, record)
    } else if let Some(token) = environment_token {
        (
            token.clone(),
            SavedAuth {
                server: server.clone(),
                token: Some(token),
                token_file: None,
            },
        )
    } else if let Some(path) = environment_file {
        let token = read_token_file(&path)?;
        (
            token,
            SavedAuth {
                server: server.clone(),
                token: None,
                token_file: Some(stored_token_file(&path)?),
            },
        )
    } else {
        eprintln!("The token will be saved in your private config file, without encryption.");
        let token = rpassword::prompt_password("API token: ")
            .context("could not read API token from terminal")?;
        let token = validate_token(token)?;
        (
            token.clone(),
            SavedAuth {
                server: server.clone(),
                token: Some(token),
                token_file: None,
            },
        )
    };

    let api = ApiClient::new(server.clone(), Some(token)).context("could not create API client")?;
    let profile = api.request(reqwest::Method::GET, "user", None).await?;
    persist_saved(&record)?;
    Ok(json!({
        "server": server.as_str(),
        "status": "logged_in",
        "credential_file": credential_path()?,
        "storage": if record.token_file.is_some() { "token-file-reference" } else { "plaintext-file" },
        "user": profile,
    }))
}

async fn status(cli: &Cli) -> Result<Value> {
    let resolved = resolve(cli)?;
    let token = resolved
        .token
        .ok_or_else(|| anyhow::anyhow!("no API token is configured for {}", resolved.server))?;
    let api = ApiClient::new(resolved.server.clone(), Some(token))?;
    let profile = api.request(reqwest::Method::GET, "user", None).await?;
    Ok(
        json!({"server": resolved.server.as_str(), "token_source": resolved.source.label(), "user": profile}),
    )
}

fn logout() -> Result<Value> {
    let path = credential_path()?;
    check_config_directory(&path)?;
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() => bail!(
            "refusing to remove symlink credential file {}",
            path.display()
        ),
        Ok(_) => fs::remove_file(&path)
            .with_context(|| format!("could not remove saved login {}", path.display()))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("could not inspect {}", path.display()));
        }
    }
    Ok(json!({
        "status": "saved_login_removed",
        "note": "Environment credentials and external token files are unchanged; server tokens are not revoked.",
    }))
}

fn explicit_token(cli: &Cli) -> Result<Option<(String, TokenSource)>> {
    let count =
        cli.token.is_some() as usize + cli.token_file.is_some() as usize + cli.token_stdin as usize;
    if count > 1 {
        bail!("choose exactly one explicit token source");
    }
    if let Some(token) = cli.token.as_deref() {
        return Ok(Some((
            validate_token(token.to_owned())?,
            TokenSource::ExplicitToken,
        )));
    }
    if cli.token_stdin {
        return Ok(Some((read_stdin_token()?, TokenSource::ExplicitStdin)));
    }
    if let Some(path) = cli.token_file.as_deref() {
        if path == Path::new("-") {
            return Ok(Some((read_stdin_token()?, TokenSource::ExplicitStdin)));
        }
        return Ok(Some((read_token_file(path)?, TokenSource::ExplicitFile)));
    }
    Ok(None)
}

fn saved_token_for_origin(saved: SavedAuth, server: &Url) -> Result<(Option<String>, TokenSource)> {
    if saved.server.origin() != server.origin() {
        return Ok((None, TokenSource::None));
    }
    if let Some(token) = saved.token {
        return Ok((Some(token), TokenSource::SavedToken));
    }
    if let Some(path) = saved.token_file {
        return Ok((Some(read_token_file(&path)?), TokenSource::SavedFile));
    }
    Ok((None, TokenSource::None))
}

fn environment_server() -> Result<Option<Url>> {
    let Some(value) = std::env::var_os("GITADEL_SERVER") else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("GITADEL_SERVER is not valid UTF-8"))?;
    Ok(Some(validate_origin(
        Url::parse(value).context("GITADEL_SERVER is not a valid URL")?,
    )?))
}

fn environment_token() -> Result<Option<String>> {
    std::env::var_os("GITADEL_TOKEN")
        .map(|value| validate_token(value.to_string_lossy().into_owned()))
        .transpose()
}

fn environment_file_path() -> Option<PathBuf> {
    std::env::var_os("GITADEL_TOKEN_FILE").map(PathBuf::from)
}

fn environment_file_token() -> Result<Option<String>> {
    environment_file_path()
        .map(|path| read_token_file(&path))
        .transpose()
}

fn default_server() -> Url {
    Url::parse(DEFAULT_SERVER).expect("static default server URL must parse")
}

fn validate_token(token: String) -> Result<String> {
    let token = token.trim();
    if token.is_empty() {
        bail!("API token is empty");
    }
    if !token.bytes().all(|byte| byte.is_ascii_graphic()) {
        bail!("API token must contain only printable non-whitespace ASCII characters");
    }
    Ok(token.to_owned())
}

fn read_stdin_token() -> Result<String> {
    let mut token = String::new();
    io::stdin()
        .read_to_string(&mut token)
        .context("could not read API token from stdin")?;
    validate_token(token)
}

fn read_token_file(path: &Path) -> Result<String> {
    let token = fs::read_to_string(path)
        .with_context(|| format!("could not read token file {}", path.display()))?;
    validate_token(token)
}

fn prompt_server(default: Url) -> Result<Url> {
    eprint!("Server [{}]: ", default);
    io::stderr()
        .flush()
        .context("could not flush server prompt")?;
    let mut value = String::new();
    io::stdin()
        .read_line(&mut value)
        .context("could not read server from terminal")?;
    let value = value.trim();
    if value.is_empty() {
        return Ok(default);
    }
    validate_origin(Url::parse(value).context("server is not a valid URL")?)
}

fn stored_token_file(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_owned());
    }
    Ok(std::env::current_dir()
        .context("could not determine current directory")?
        .join(path))
}

fn credential_path() -> Result<PathBuf> {
    let directory = dirs::config_dir()
        .filter(|directory| directory.is_absolute())
        .ok_or_else(|| {
            anyhow::anyhow!("could not determine an absolute per-user config directory")
        })?;
    Ok(directory.join("gitadel").join("auth.json"))
}

fn check_config_directory(path: &Path) -> Result<()> {
    if let Some(directory) = path.parent() {
        match fs::symlink_metadata(directory) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                bail!(
                    "refusing to use symlink config directory {}",
                    directory.display()
                );
            }
            Ok(metadata) if !metadata.is_dir() => {
                bail!("config directory is not a directory");
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("could not inspect config directory"),
        }
    }
    Ok(())
}

fn load_saved() -> Result<Option<SavedAuth>> {
    let path = match credential_path() {
        Ok(path) => path,
        Err(_) => return Ok(None),
    };
    check_config_directory(&path)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
            bail!("saved login must be a regular file, not a symlink");
        }
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("could not inspect saved login"),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            bail!("saved login is accessible to other users; set its permissions to 0600");
        }
    }
    let text = fs::read_to_string(&path).context("could not read saved login")?;
    Ok(Some(parse_saved(&text)?))
}

fn parse_saved(text: &str) -> Result<SavedAuth> {
    let value: Value = serde_json::from_str(text).context("saved login is not valid JSON")?;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("saved login is not an object"))?;
    let server = object
        .get("server")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("saved login has no server"))?;
    let server = validate_origin(Url::parse(server).context("saved login server is invalid")?)?;
    let token = object
        .get("token")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let token_file = object
        .get("token_file")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    if token.is_some() == token_file.is_some() {
        bail!("saved login must contain exactly one token source");
    }
    if token_file.as_ref().is_some_and(|path| !path.is_absolute()) {
        bail!("saved token file must be an absolute path");
    }
    Ok(SavedAuth {
        server,
        token: token.map(validate_token).transpose()?,
        token_file,
    })
}

fn persist_saved(saved: &SavedAuth) -> Result<()> {
    let path = credential_path()?;
    let directory = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("credential path has no parent directory"))?;
    check_config_directory(&path)?;
    fs::create_dir_all(directory)
        .with_context(|| format!("could not create config directory {}", directory.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).with_context(|| {
            format!("could not secure config directory {}", directory.display())
        })?;
    }
    if let Ok(metadata) = fs::symlink_metadata(&path)
        && metadata.file_type().is_symlink()
    {
        bail!(
            "refusing to replace symlink credential file {}",
            path.display()
        );
    }
    let value = if let Some(token) = saved.token.as_deref() {
        json!({"server": saved.server.as_str(), "token": token})
    } else if let Some(path) = saved.token_file.as_deref() {
        let path = path
            .to_str()
            .context("token file path is not valid UTF-8")?;
        json!({"server": saved.server.as_str(), "token_file": path})
    } else {
        bail!("cannot save an empty login");
    };
    let data = serde_json::to_vec_pretty(&value)?;
    let temporary = directory.join(format!(".auth.json.{}.tmp", Uuid::new_v4()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let result = (|| -> Result<()> {
        let mut file = options
            .open(&temporary)
            .with_context(|| format!("could not create {}", temporary.display()))?;
        file.write_all(&data)
            .with_context(|| format!("could not write {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("could not flush {}", temporary.display()))?;
        drop(file);
        if let Ok(metadata) = fs::symlink_metadata(&path)
            && metadata.file_type().is_symlink()
        {
            bail!(
                "refusing to replace symlink credential file {}",
                path.display()
            );
        }
        fs::rename(&temporary, &path)
            .with_context(|| format!("could not publish {}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_token_is_bound_to_normalized_origin() {
        let saved =
            parse_saved(r#"{"server":"https://GIT.EXAMPLE.TEST:443/","token":"secret"}"#).unwrap();
        let origin = Url::parse("https://git.example.test").unwrap();
        let (token, source) = saved_token_for_origin(saved, &origin).unwrap();
        assert_eq!(token.as_deref(), Some("secret"));
        assert_eq!(source, TokenSource::SavedToken);
    }
    #[test]
    fn saved_token_is_not_forwarded_to_another_origin() {
        let saved =
            parse_saved(r#"{"server":"https://git.example.test","token":"secret"}"#).unwrap();
        let origin = Url::parse("https://other.example.test").unwrap();
        let (token, source) = saved_token_for_origin(saved, &origin).unwrap();
        assert!(token.is_none());
        assert_eq!(source, TokenSource::None);
    }
    #[test]
    fn saved_token_file_is_not_opened_for_another_origin() {
        let saved = SavedAuth {
            server: Url::parse("https://git.example.test").unwrap(),
            token: None,
            token_file: Some(std::env::temp_dir().join(Uuid::new_v4().to_string())),
        };
        let other = Url::parse("http://git.example.test").unwrap();
        assert!(saved_token_for_origin(saved, &other).unwrap().0.is_none());
    }
    #[test]
    fn saved_login_rejects_multiple_secret_sources() {
        assert!(parse_saved(r#"{"server":"https://git.example.test","token":"secret","token_file":"/run/secrets/token"}"#).is_err());
    }
}
