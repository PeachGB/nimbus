use futures::StreamExt;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::process::Output;
use tokio::{io::AsyncWriteExt, process::Command};
use tokio_util::io::ReaderStream;

use crate::{
    VaultResult,
    error::VaultError,
    object::{Metadata, Object, ObjectId, validate_name},
    origin::{ByteStream, Origin},
};

/// Makes `value` a single literal word for `sh`. Values made only of characters the shell treats
/// as plain (`[A-Za-z0-9_@%+:,./-]`) are left as they are, so ordinary names and paths read the
/// same in the expanded command. Anything else is wrapped in single quotes, with embedded `'`
/// written as `'\''`.
///
/// Every placeholder goes through this. Names and ids come from the *other* side of a transfer,
/// and pasting them raw into `sh -c` let a file named `x'; rm -rf ~; echo '` run commands.
/// Because a quoted value is still one shell word, placeholders compose inside paths
/// (`{root}/data/{id}`) but must not be wrapped in quotes in the template itself.
fn sh_quote(value: &str) -> std::borrow::Cow<'_, str> {
    let plain = |c: char| c.is_ascii_alphanumeric() || "_@%+:,./-".contains(c);
    if !value.is_empty() && value.chars().all(plain) {
        std::borrow::Cow::Borrowed(value)
    } else {
        std::borrow::Cow::Owned(format!("'{}'", value.replace('\'', r"'\''")))
    }
}

/// Expands every `{key}` in `template` that `vars` has, shell-quoting each value, in **one
/// pass**: text that came from a value is never scanned again. Replacing one placeholder at a
/// time did re-scan it, so a remote object named `{x}` whose metadata carried
/// `x = "$(cmd)"` expanded to `''$(cmd)''`, which the quotes no longer protect, and ran
/// `cmd`. Unknown `{keys}` are left as they are.
fn render(template: &str, vars: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find(['}', '{']) {
            Some(close) if after.as_bytes()[close] == b'}' => {
                let key = &after[..close];
                match vars.get(key) {
                    Some(value) => out.push_str(&sh_quote(value)),
                    None => {
                        out.push('{');
                        out.push_str(key);
                        out.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            // A `{` with no matching `}` before the next `{` is literal text (JSON in an
            // `echo '...'` template, say).
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// An [`crate::origin::Origin`] backed by a shell command per operation. Each command is a
/// `{placeholder}`-templated string, substituted with the object id/name/metadata plus any
/// `extra_vars`; `list`/`get` expect the command's stdout to be JSON matching `Object`,
/// `fetch`/`send` stream the payload over stdout/stdin.
///
/// # Examples
///
/// ```
/// use nimbus_vault::{object::ObjectId, origin::{Origin, command::OriginCommand}};
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let origin = OriginCommand::new(
///     "printf hello".to_string(),            // fetch_cmd
///     "echo '[]'".to_string(),                // list_cmd
///     r#"echo '{"Leaf":{"name":"f","id":"{id}","meta":{"size":null,"content_type":null,"modified":null,"extra":{}}}}'"#.to_string(), // get_cmd
///     "true".to_string(),                     // put_cmd
///     "true".to_string(),                     // send_cmd
///     "true".to_string(),                     // delete_cmd
///     None,                                   // extra_vars
/// );
///
/// let object = origin.get(&ObjectId::from("f1")).await?;
/// assert_eq!(object.get_id().as_str(), "f1"); // `{id}` was substituted into the template
/// # Ok(())
/// # }
/// ```
///
/// Declaratively, via `[origin_config]` in a vault's TOML config:
///
/// ```toml
/// [origin_config]
/// type = "command"
/// list_cmd   = "ls {root}"
/// fetch_cmd  = "cat {root}/{id}"
/// get_cmd    = "stat {root}/{id}"
/// put_cmd    = "touch {root}/{id}"
/// send_cmd   = "tee {root}/{id}"
/// delete_cmd = "rm {root}/{id}"
///
/// [origin_config.extras]
/// root = "/srv/data"
/// ```
pub struct OriginCommand {
    fetch_cmd: String,
    list_cmd: String,
    get_cmd: String,

    put_cmd: String,
    send_cmd: String,
    delete_cmd: String,
    /// Optional native move; see [`OriginCommand::with_rename_cmd`].
    rename_cmd: Option<String>,
    extra_vars: HashMap<String, String>,
}

/// Identifies which of `OriginCommand`'s configured command templates to run.
pub enum CmdType {
    /// Runs `fetch_cmd`.
    Fetch,
    /// Runs `list_cmd`.
    List,
    /// Runs `get_cmd`.
    Get,

    /// Runs `put_cmd`.
    Put,
    /// Runs `send_cmd`.
    Send,

    /// Runs `delete_cmd`.
    Delete,
}
impl CmdType {
    /// The config field name this variant corresponds to (e.g. `"fetch_cmd"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            CmdType::Fetch => crate::FETCH_CMD_FIELD,
            CmdType::List => crate::LIST_CMD_FIELD,
            CmdType::Get => crate::GET_CMD_FIELD,
            CmdType::Put => crate::PUT_CMD_FIELD,
            CmdType::Send => crate::SEND_CMD_FIELD,
            CmdType::Delete => crate::DELETE_CMD_FIELD,
        }
    }
}

/// Formats as the config field name, so `to_string()` (via the blanket [`ToString`]) and `{}`
/// both give what [`CmdType::as_str`] returns.
impl std::fmt::Display for CmdType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
impl OriginCommand {
    /// Builds an `OriginCommand` from one command template per operation, plus optional extra
    /// `{placeholder}` substitutions shared by every template.
    pub fn new(
        fetch_cmd: String,
        list_cmd: String,
        get_cmd: String,

        put_cmd: String,
        send_cmd: String,
        delete_cmd: String,
        extra_vars: Option<HashMap<String, String>>,
    ) -> Self {
        OriginCommand {
            fetch_cmd,
            list_cmd,
            get_cmd,
            put_cmd,
            send_cmd,
            delete_cmd,
            rename_cmd: None,
            extra_vars: extra_vars.unwrap_or_default(),
        }
    }

    /// Enables [`Origin::rename`] through `rename_cmd`, templated with `{id}` (what to move),
    /// `{destination}` (the new parent's id), `{name}` (the new name) and the configured
    /// extras. Without it, moves fall back to copy-then-delete. Make the command atomic if you
    /// can, e.g. a single `mv`.
    pub fn with_rename_cmd(mut self, rename_cmd: Option<String>) -> Self {
        self.rename_cmd = rename_cmd;
        self
    }
    #[cfg(test)]
    fn interpolate_one<I: AsRef<str>>(template: &mut String, key: &str, val: I) {
        let vars = HashMap::from([(key.to_string(), val.as_ref().to_string())]);
        *template = render(template, &vars);
    }
    #[cfg(test)]
    fn interpolate(template: &mut String, vars: &HashMap<String, String>) {
        *template = render(template, vars);
    }
    async fn bootstrap_cmd_id(&self, t: &CmdType, id: &ObjectId) -> String {
        let cmd = match t {
            CmdType::Fetch => self.fetch_cmd.clone(),
            CmdType::List => self.list_cmd.clone(),
            CmdType::Get => self.get_cmd.clone(),
            CmdType::Put => self.put_cmd.clone(),
            CmdType::Delete => self.delete_cmd.clone(),
            CmdType::Send => self.send_cmd.clone(),
        };
        let mut vars = self.extra_vars.clone();
        vars.insert(crate::PLACEHOLDER_ID.to_string(), id.to_string());
        render(&cmd, &vars)
    }
    /// Builds a `put`/`send` command template, substituting the object's own fields plus the
    /// configured `extra_vars`.
    ///
    /// The extras matter as much here as in [`Self::bootstrap_cmd_id`]: without them a config
    /// whose `put_cmd`/`send_cmd` reference a `{helper}` or `{root}` placeholder would run with
    /// the literal braces still in the string.
    async fn bootstrap_cmd_object(&self, t: &CmdType, object: &Object) -> VaultResult<String> {
        self.bootstrap_cmd_object_with(t, object, &HashMap::new())
            .await
    }
    /// [`Self::bootstrap_cmd_object`] plus `call_vars`: values that belong to this one call, such
    /// as `put`'s `{destination}`. Passed in rather than stored on `self`, so concurrent calls
    /// can't see each other's values.
    async fn bootstrap_cmd_object_with(
        &self,
        t: &CmdType,
        object: &Object,
        call_vars: &HashMap<String, String>,
    ) -> VaultResult<String> {
        let cmd = match t {
            CmdType::Put | CmdType::Send => {
                let Some(Metadata {
                    size,
                    content_type,
                    modified,
                    extra,
                }) = object.get_meta()
                else {
                    return Err(VaultError::Unreachable);
                };
                let size = match size {
                    Some(s) => s.to_string(),
                    None => 0.to_string(),
                };
                let content_type =
                    content_type.unwrap_or(String::from(crate::UNKNOWN_CONTENT_TYPE));
                let modified = match modified {
                    Some(date) => date.to_string(),
                    None => String::from(""),
                };

                let kind = match object {
                    Object::Branch { .. } | Object::Root { .. } => crate::OBJECT_KIND_BRANCH,
                    Object::Leaf { .. } => crate::OBJECT_KIND_LEAF,
                };

                let template = match t {
                    CmdType::Put => &self.put_cmd,
                    CmdType::Send => &self.send_cmd,
                    _ => unreachable!(),
                };
                // Lowest precedence first: config extras, then the object's own extras (a
                // per-object value beats the config default), then per-call values, then the
                // built-in placeholders, which nothing can override.
                let mut vars = self.extra_vars.clone();
                vars.extend(extra);
                vars.extend(call_vars.clone());
                for (key, value) in [
                    (crate::PLACEHOLDER_ID, object.get_id().to_string()),
                    (crate::PLACEHOLDER_NAME, object.get_name()),
                    (crate::PLACEHOLDER_SIZE, size),
                    (crate::PLACEHOLDER_CONTENT_TYPE, content_type),
                    (crate::PLACEHOLDER_MODIFIED, modified),
                    (crate::PLACEHOLDER_KIND, kind.to_string()),
                ] {
                    vars.insert(key.to_string(), value);
                }
                render(template, &vars)
            }
            _ => return Err(VaultError::InvalidMethodCall),
        };
        Ok(cmd)
    }

    async fn cmd_json<T: DeserializeOwned>(&self, t: CmdType, id: &ObjectId) -> VaultResult<T> {
        let output = self.cmd(t, id).await?;
        serde_json::from_slice(&output.stdout)
            .map_err(|e| VaultError::Generic(format!("command output was not valid JSON: {}", e)))
    }
    async fn cmd(&self, t: CmdType, id: &ObjectId) -> VaultResult<Output> {
        let cmd = self.bootstrap_cmd_id(&t, id).await;
        let output = Command::new("sh").arg("-c").arg(&cmd).output().await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(VaultError::Generic(format!(
                "{} failed for {}: {}",
                t,
                id.as_str(),
                stderr.trim()
            )));
        }
        Ok(output)
    }
}
#[async_trait::async_trait]
impl Origin for OriginCommand {
    async fn fetch(&self, id: &ObjectId) -> VaultResult<ByteStream> {
        use tokio::io::AsyncReadExt;

        let cmd = self.bootstrap_cmd_id(&CmdType::Fetch, id).await;
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // A consumer that stops reading early (or fails on a later chunk) drops the
            // stream, and with it the child — don't leave it running or as a zombie.
            .kill_on_drop(true)
            .spawn()?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| VaultError::Generic("failed to open stdout".into()))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| VaultError::Generic("failed to open stderr".into()))?;
        // Drained concurrently: a command that writes a lot to stderr would otherwise block on
        // a full pipe and never finish its stdout.
        let stderr = tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf).await;
            buf
        });

        // The exit status only exists once stdout is exhausted, so it's checked as one last
        // step of the stream. Without it a fetch_cmd that died half-way ended the stream
        // "cleanly", and the partial output was sent on as the object's complete contents.
        let id = id.to_string();
        let status = futures::stream::once(async move {
            let status = child.wait().await?;
            if status.success() {
                return Ok(None);
            }
            let stderr = stderr.await.unwrap_or_default();
            Err(VaultError::Generic(format!(
                "fetch_cmd failed for {id} ({status}): {}",
                String::from_utf8_lossy(&stderr).trim()
            )))
        })
        .filter_map(
            |outcome: VaultResult<Option<bytes::Bytes>>| async move { outcome.transpose() },
        );

        let body = ReaderStream::new(stdout).map(|chunk| chunk.map_err(VaultError::from));
        Ok(Box::pin(body.chain(status)))
    }
    async fn list(&self, id: &ObjectId) -> VaultResult<Vec<Object>> {
        let objects: Vec<Object> = self.cmd_json(CmdType::List, id).await?;
        Ok(objects)
    }
    async fn get(&self, id: &ObjectId) -> VaultResult<Object> {
        let object: Object = self.cmd_json(CmdType::Get, id).await?;
        Ok(object)
    }
    async fn put(&self, obj: &mut Object, destination: &ObjectId) -> VaultResult<Object> {
        validate_name(&obj.get_name())?;
        // Per call, never stored on `self`: shared state let concurrent puts (the daemon serves
        // requests in parallel) run with each other's destination.
        let call_vars = HashMap::from([(
            crate::PLACEHOLDER_DESTINATION.to_string(),
            destination.to_string(),
        )]);
        let cmd = self
            .bootstrap_cmd_object_with(&CmdType::Put, obj, &call_vars)
            .await?;
        let output = Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .output()
            .await
            .map_err(|e| VaultError::Generic(format!("Failed to execute command: {}", e)))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(VaultError::Generic(format!(
                "put_cmd failed for {}: {}",
                obj.get_id().as_str(),
                stderr.trim()
            )));
        }
        // Trimmed like `OriginHTTP::put`: destination `/` plus `a.txt` is `/a.txt`, not `//a.txt`.
        let new_id = ObjectId::from(format!(
            "{}/{}",
            destination.as_str().trim_end_matches('/'),
            obj.get_name()
        ));
        self.get(&new_id).await
    }
    async fn send(&self, object: &Object, mut payload: ByteStream) -> VaultResult<()> {
        let cmd = self.bootstrap_cmd_object(&CmdType::Send, object).await?;
        let mut child = Command::new("sh")
            // `-c` is what makes this a shell string like every other template; without it `sh`
            // treats the whole thing as a filename to execute.
            .arg("-c")
            .arg(&cmd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| VaultError::Generic("failed to open send_cmd stdin".into()))?;

        let written = async {
            while let Some(chunk) = payload.next().await {
                stdin.write_all(&chunk?).await?;
            }
            Ok::<(), VaultError>(())
        }
        .await;
        if let Err(e) = written {
            // Kill *before* stdin closes. Closing it first would look like a normal end of
            // input to the script, which would then commit the partial payload as the whole
            // object.
            let _ = child.kill().await;
            return Err(e);
        }
        drop(stdin);
        let output = child.wait_with_output().await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(VaultError::Generic(format!(
                "send_cmd failed for {}: {}",
                object.get_id().as_str(),
                stderr.trim()
            )));
        }
        Ok(())
    }
    async fn delete(&self, id: &ObjectId) -> VaultResult<()> {
        let _output = self.cmd(CmdType::Delete, id).await?;
        Ok(())
    }
    async fn rename(
        &self,
        id: &ObjectId,
        new_parent: &ObjectId,
        new_name: &str,
    ) -> VaultResult<Object> {
        let Some(template) = &self.rename_cmd else {
            return Err(VaultError::Unsupported(
                "rename (no rename_cmd configured)".into(),
            ));
        };
        validate_name(new_name)?;
        let mut vars = self.extra_vars.clone();
        vars.insert(crate::PLACEHOLDER_ID.to_string(), id.to_string());
        vars.insert(
            crate::PLACEHOLDER_DESTINATION.to_string(),
            new_parent.to_string(),
        );
        vars.insert(crate::PLACEHOLDER_NAME.to_string(), new_name.to_string());
        let cmd = render(template, &vars);

        let output = Command::new("sh").arg("-c").arg(&cmd).output().await?;
        if !output.status.success() {
            return Err(VaultError::Generic(format!(
                "rename_cmd failed for {}: {}",
                id.as_str(),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let new_id = ObjectId::from(format!(
            "{}/{}",
            new_parent.as_str().trim_end_matches('/'),
            new_name
        ));
        self.get(&new_id).await
    }
}

#[cfg(test)]
#[path = "tests/command.rs"]
mod tests;
