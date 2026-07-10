use futures::StreamExt;
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

pub struct OriginCommand {
    fetch_cmd: String,
    list_cmd: String,
    get_cmd: String,

    put_cmd: String,
    send_cmd: String,
    delete_cmd: String,
    extra_vars: HashMap<String, String>,
}
pub enum CmdType {
    Fetch,
    List,
    Get,

    Put,
    Send,

    Delete,
}
impl CmdType {
    pub fn as_str(&self) -> &'static str {
        match self {
            CmdType::Fetch => "fetch_cmd",
            CmdType::List => "list_cmd",
            CmdType::Get => "get_cmd",
            CmdType::Put => "put_cmd",
            CmdType::Send => "send_cmd",
            CmdType::Delete => "delete_cmd",
        }
    }
    pub fn to_string(&self) -> String {
        self.as_str().to_string()
    }
}
impl OriginCommand {
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
            extra_vars: extra_vars.unwrap_or_default(),
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
    fn bootstrap_cmd_id(&self, t: &CmdType, id: &ObjectId) -> String {
        let mut cmd = match t {
            CmdType::Fetch => self.fetch_cmd.replace("{id}", id.as_str()),
            CmdType::List => self.list_cmd.replace("{id}", id.as_str()),
            CmdType::Get => self.get_cmd.replace("{id}", id.as_str()),
            CmdType::Put => self.put_cmd.replace("{id}", id.as_str()),
            CmdType::Delete => self.delete_cmd.replace("{id}", id.as_str()),
            CmdType::Send => self.send_cmd.replace("{id}", id.as_str()),
        };

        Self::interpolate(&mut cmd, &self.extra_vars);

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
                let content_type = content_type.unwrap_or(String::from("unknown"));
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
                Self::interpolate_one(c, "id", object.get_id());
                Self::interpolate_one(c, "name", object.get_name());
                Self::interpolate_one(c, "size", size);
                Self::interpolate_one(c, "content_type", content_type);
                Self::interpolate_one(c, "modified", modified);
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
        let cmd = self.bootstrap_cmd_id(&t, id);
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
        let cmd = self.bootstrap_cmd_id(&CmdType::Fetch, id);
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .stdout(std::process::Stdio::piped())
            .spawn()?;

        let mut stdout = child
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
    async fn put(&self, obj: &Object) -> VaultResult<()> {
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

        Ok(())
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
mod tests {
    use super::*;
    use crate::object::Metadata;

    fn make_command(overrides: impl FnOnce(&mut OriginCommand)) -> OriginCommand {
        let mut cmd = OriginCommand {
            fetch_cmd: "echo fetch {id}".to_string(),
            list_cmd: "echo list {id}".to_string(),
            get_cmd: "echo get {id}".to_string(),
            put_cmd: "echo put {id} {name}".to_string(),
            send_cmd: "echo send {id}".to_string(),
            delete_cmd: "echo delete {id}".to_string(),
            extra_vars: HashMap::new(),
        };
        overrides(&mut cmd);
        cmd
    }

    #[test]
    fn cmd_type_as_str_maps_variants() {
        assert_eq!(CmdType::Fetch.as_str(), "fetch_cmd");
        assert_eq!(CmdType::List.as_str(), "list_cmd");
        assert_eq!(CmdType::Get.as_str(), "get_cmd");
        assert_eq!(CmdType::Put.as_str(), "put_cmd");
        assert_eq!(CmdType::Send.as_str(), "send_cmd");
        assert_eq!(CmdType::Delete.as_str(), "delete_cmd");
        assert_eq!(CmdType::Fetch.to_string(), "fetch_cmd");
    }

    #[test]
    fn interpolate_one_replaces_placeholder() {
        let mut template = "hello {name}!".to_string();
        OriginCommand::interpolate_one(&mut template, "name", "world");
        assert_eq!(template, "hello world!");
    }

    #[test]
    fn interpolate_replaces_all_vars() {
        let mut template = "{a} and {b}".to_string();
        let mut vars = HashMap::new();
        vars.insert("a".to_string(), "1".to_string());
        vars.insert("b".to_string(), "2".to_string());
        OriginCommand::interpolate(&mut template, &vars);
        assert_eq!(template, "1 and 2");
    }

    #[test]
    fn bootstrap_cmd_id_substitutes_id_and_extra_vars() {
        let mut extra = HashMap::new();
        extra.insert("token".to_string(), "secret".to_string());
        let cmd = make_command(|c| {
            c.fetch_cmd = "fetch --id {id} --token {token}".to_string();
            c.extra_vars = extra;
        });

        let result = cmd.bootstrap_cmd_id(&CmdType::Fetch, &ObjectId::from("obj1"));
        assert_eq!(result, "fetch --id obj1 --token secret");
    }

    #[test]
    fn bootstrap_cmd_object_substitutes_metadata_fields() {
        let cmd = make_command(|c| {
            c.put_cmd = "put {id} {name} {size} {content_type}".to_string();
        });
        let mut meta = Metadata::new();
        meta.set_size(10).set_content_type("text/plain".to_string());
        let object = Object::Leaf {
            name: "file.txt".to_string(),
            id: ObjectId::from("f1"),
            meta,
        };

        let result = cmd.bootstrap_cmd_object(&CmdType::Put, &object).unwrap();
        assert_eq!(result, "put f1 file.txt 10 text/plain");
    }

    #[test]
    fn bootstrap_cmd_object_defaults_missing_metadata_fields() {
        let cmd = make_command(|c| {
            c.put_cmd = "put {size} {content_type} {modified}".to_string();
        });
        let object = Object::Leaf {
            name: "file.txt".to_string(),
            id: ObjectId::from("f1"),
            meta: Metadata::new(),
        };

        let result = cmd.bootstrap_cmd_object(&CmdType::Put, &object).unwrap();
        assert_eq!(result, "put 0 unknown ");
    }

    #[test]
    fn bootstrap_cmd_object_uses_send_cmd_template_for_send() {
        let cmd = make_command(|c| {
            c.put_cmd = "put {id}".to_string();
            c.send_cmd = "send {id}".to_string();
        });
        let object = Object::Leaf {
            name: "file.txt".to_string(),
            id: ObjectId::from("f1"),
            meta: Metadata::new(),
        };

        let result = cmd.bootstrap_cmd_object(&CmdType::Send, &object).unwrap();
        assert_eq!(result, "send f1");
    }

    #[test]
    fn bootstrap_cmd_object_rejects_non_put_send_types() {
        let cmd = make_command(|_| {});
        let object = Object::Leaf {
            name: "file.txt".to_string(),
            id: ObjectId::from("f1"),
            meta: Metadata::new(),
        };
        let result = cmd.bootstrap_cmd_object(&CmdType::Get, &object);
        assert!(matches!(result, Err(VaultError::InvalidMethodCall)));
    }

    #[test]
    fn bootstrap_cmd_object_errors_on_root_object() {
        let cmd = make_command(|_| {});
        let object = Object::root();
        let result = cmd.bootstrap_cmd_object(&CmdType::Put, &object);
        assert!(matches!(result, Err(VaultError::Unreachable)));
    }

    #[tokio::test]
    async fn get_parses_json_output_from_command() {
        let cmd = make_command(|c| {
            c.get_cmd = r#"echo '{"Leaf":{"name":"file","id":"f1","meta":{"size":null,"content_type":null,"modified":null,"extra":{}}}}'"#.to_string();
        });

        let object = cmd.get(&ObjectId::from("f1")).await.unwrap();
        match object {
            Object::Leaf { name, id, .. } => {
                assert_eq!(name, "file");
                assert_eq!(id.as_str(), "f1");
            }
            _ => panic!("expected leaf"),
        }
    }

    #[tokio::test]
    async fn list_parses_json_array_output() {
        let cmd = make_command(|c| {
            c.list_cmd = r#"echo '[{"Leaf":{"name":"a","id":"a","meta":{"size":null,"content_type":null,"modified":null,"extra":{}}}}]'"#.to_string();
        });

        let objects = cmd.list(&ObjectId::from("dir")).await.unwrap();
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].get_name(), "a");
    }

    #[tokio::test]
    async fn cmd_returns_generic_error_on_nonzero_exit() {
        let cmd = make_command(|c| {
            c.get_cmd = "sh -c 'exit 1'".to_string();
        });
        let result = cmd.get(&ObjectId::from("f1")).await;
        assert!(matches!(result, Err(VaultError::Generic(_))));
    }

    #[tokio::test]
    async fn put_succeeds_on_zero_exit() {
        let cmd = make_command(|c| {
            c.put_cmd = "true".to_string();
        });
        let object = Object::Leaf {
            name: "file.txt".to_string(),
            id: ObjectId::from("f1"),
            meta: Metadata::new(),
        };
        cmd.put(&object).await.unwrap();
    }

    #[tokio::test]
    async fn put_errors_on_command_failure() {
        let cmd = make_command(|c| {
            c.put_cmd = "false".to_string();
        });
        let object = Object::Leaf {
            name: "file.txt".to_string(),
            id: ObjectId::from("f1"),
            meta: Metadata::new(),
        };
        let result = cmd.put(&object).await;
        assert!(matches!(result, Err(VaultError::Generic(_))));
    }

    #[tokio::test]
    async fn delete_runs_configured_command() {
        let cmd = make_command(|c| {
            c.delete_cmd = "true".to_string();
        });
        cmd.delete(&ObjectId::from("f1")).await.unwrap();
    }

    #[tokio::test]
    async fn fetch_streams_command_stdout() {
        let cmd = make_command(|c| {
            c.fetch_cmd = "printf hello".to_string();
        });
        let mut stream = cmd.fetch(&ObjectId::from("f1")).await.unwrap();
        let mut collected = Vec::new();
        while let Some(chunk) = stream.next().await {
            collected.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(collected, b"hello");
    }
}
