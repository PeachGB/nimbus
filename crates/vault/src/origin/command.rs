use futures::{StreamExt, lock::Mutex};
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::process::Output;
use tokio::{io::AsyncWriteExt, process::Command};
use tokio_util::io::ReaderStream;

use crate::{
    VaultResult,
    error::VaultError,
    object::{Metadata, Object, ObjectId},
    origin::{ByteStream, Origin},
};

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
    extra_vars: Mutex<HashMap<String, String>>,
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
    /// Owned version of [`CmdType::as_str`].
    pub fn to_string(&self) -> String {
        self.as_str().to_string()
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
            extra_vars: Mutex::new(extra_vars.unwrap_or_default()),
        }
    }
    fn interpolate_one<I: AsRef<str>>(template: &mut String, key: &str, val: I) {
        *template = template.replace(&format!("{{{key}}}"), val.as_ref());
    }
    fn interpolate(template: &mut String, vars: &HashMap<String, String>) {
        for (k, v) in vars {
            *template = template.replace(&format!("{{{k}}}"), v);
        }
    }
    async fn bootstrap_cmd_id(&self, t: &CmdType, id: &ObjectId) -> String {
        let mut cmd = match t {
            CmdType::Fetch => self.fetch_cmd.clone(),
            CmdType::List => self.list_cmd.clone(),
            CmdType::Get => self.get_cmd.clone(),
            CmdType::Put => self.put_cmd.clone(),
            CmdType::Delete => self.delete_cmd.clone(),
            CmdType::Send => self.send_cmd.clone(),
        };
        Self::interpolate_one(&mut cmd, crate::PLACEHOLDER_ID, id.as_str());
        let vars = self.extra_vars.lock().await;
        Self::interpolate(&mut cmd, &vars);

        cmd
    }
    fn bootstrap_cmd_object(&self, t: &CmdType, object: &Object) -> VaultResult<String> {
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

                let mut command = match t {
                    CmdType::Put => self.put_cmd.clone(),
                    CmdType::Send => self.send_cmd.clone(),
                    _ => unreachable!(),
                };
                let c = &mut command;
                Self::interpolate_one(c, crate::PLACEHOLDER_ID, object.get_id());
                Self::interpolate_one(c, crate::PLACEHOLDER_NAME, object.get_name());
                Self::interpolate_one(c, crate::PLACEHOLDER_SIZE, size);
                Self::interpolate_one(c, crate::PLACEHOLDER_CONTENT_TYPE, content_type);
                Self::interpolate_one(c, crate::PLACEHOLDER_MODIFIED, modified);
                Self::interpolate(c, &extra);

                command
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
                t.to_string(),
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
        let cmd = self.bootstrap_cmd_id(&CmdType::Fetch, id).await;
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .stdout(std::process::Stdio::piped())
            .spawn()?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| VaultError::Generic("failed to open stdout".into()))?;

        let stream = ReaderStream::new(stdout).map(|chunk| chunk.map_err(VaultError::from));
        Ok(Box::pin(stream))
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
        {
            let mut vars = self.extra_vars.lock().await;
            if !vars.contains_key(crate::PLACEHOLDER_DESTINATION) {
                vars.insert(
                    crate::PLACEHOLDER_DESTINATION.into(),
                    destination.to_string(),
                );
            }
        }
        let cmd = self.bootstrap_cmd_object(&CmdType::Put, obj)?;
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
        let new_id = ObjectId::from(format!("{}/{}", destination.as_str(), obj.get_name()));
        self.get(&new_id).await
    }
    async fn send(&self, object: &Object, mut payload: ByteStream) -> VaultResult<()> {
        let cmd = self.bootstrap_cmd_object(&CmdType::Send, object)?;
        let mut child = Command::new("sh")
            .arg(&cmd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| VaultError::Generic("failed to open send_cmd stdin".into()))?;

        while let Some(chunk) = payload.next().await {
            stdin.write_all(&chunk?).await?;
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
}

#[cfg(test)]
#[path = "tests/command.rs"]
mod tests;
